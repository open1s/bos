//! A durable reminder scheduler.
//!
//! The GUI owns the clock and the store owns the state: every command takes
//! `now_ms` from the caller, so nothing here reads the wall clock and the whole
//! thing is testable without waiting. A reminder is either a one-shot or a
//! repeat with **at least one minute** between deliveries, schedules persist to
//! `~/.bos/gui/schedules.json` so a restart does not lose them, and deliveries
//! land in a bounded history (the newest 200) that outlives the schedule that
//! produced them.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Fewest minutes a repeating reminder may have between deliveries. DSH's
/// scheduler is also minute-granular; a sub-minute repeat is a busy loop with a
/// clock, not a reminder.
pub(crate) const MIN_EVERY_MINUTES: u64 = 1;

/// Most reminders one store keeps. Past this, `add` is refused rather than
/// silently dropping the oldest: a schedule the user cannot see is worse than
/// an error that names the cap.
pub(crate) const MAX_SCHEDULES: usize = 50;

/// Deliveries remembered. The newest win; older ones fall off.
pub(crate) const MAX_HISTORY: usize = 200;

/// Longest reminder text, in characters.
pub(crate) const MAX_TEXT_CHARS: usize = 500;

/// The shape version of `schedules.json`.
const FILE_VERSION: u32 = 1;

/// When a reminder repeats.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Cadence {
    /// One delivery at `at_ms`, then the reminder is done.
    Once {
        /// Epoch milliseconds of the single delivery.
        at_ms: u64,
    },
    /// A delivery every `every_minutes`, the next one at `next_ms`.
    Every {
        /// Minutes between deliveries (at least [`MIN_EVERY_MINUTES`]).
        every_minutes: u64,
        /// Epoch milliseconds of the next delivery.
        next_ms: u64,
    },
}

impl Cadence {
    /// Epoch milliseconds of the next delivery.
    pub(crate) fn due_ms(&self) -> u64 {
        match self {
            Cadence::Once { at_ms } => *at_ms,
            Cadence::Every { next_ms, .. } => *next_ms,
        }
    }

    /// Whether this cadence is one the store accepts.
    fn is_valid(&self) -> bool {
        match self {
            Cadence::Once { at_ms } => *at_ms > 0,
            Cadence::Every {
                every_minutes,
                next_ms,
            } => *every_minutes >= MIN_EVERY_MINUTES && *next_ms > 0,
        }
    }
}

/// One reminder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Schedule {
    /// Stable identity, shown in the UI and used to cancel.
    pub(crate) id: String,
    /// What to remind about.
    pub(crate) text: String,
    /// When it repeats.
    pub(crate) cadence: Cadence,
    /// Epoch milliseconds it was created at.
    pub(crate) created_ms: u64,
    /// Epoch milliseconds of the last delivery, if any.
    pub(crate) last_fired_ms: Option<u64>,
    /// How many times it has been delivered.
    pub(crate) fired: u64,
}

/// One delivery, kept after the reminder that produced it is gone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Delivery {
    /// The reminder's id.
    pub(crate) schedule_id: String,
    /// The reminder's text, copied at delivery time.
    pub(crate) text: String,
    /// Epoch milliseconds it was due (not when it was noticed).
    pub(crate) at_ms: u64,
    /// Epoch milliseconds a tick noticed it. Equal to `at_ms` for a reminder
    /// delivered on time; later when the app was closed or busy, which is what
    /// lets the pane say how late instead of pretending otherwise. Files
    /// written before this field existed default to the due time.
    #[serde(default)]
    pub(crate) noticed_ms: u64,
}

/// The bounds the store enforces, handed to the UI so it can state them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Caps {
    /// Minutes a repeat needs between deliveries.
    pub(crate) min_every_minutes: u64,
    /// Reminders one store keeps.
    pub(crate) max_schedules: usize,
    /// Deliveries remembered.
    pub(crate) max_history: usize,
    /// Characters a reminder text may use.
    pub(crate) max_text_chars: usize,
}

/// Everything the Schedule pane draws, in one reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Report {
    /// The clock the reply was computed against.
    pub(crate) now_ms: u64,
    /// Reminders, in creation order.
    pub(crate) schedules: Vec<Schedule>,
    /// Deliveries, newest first.
    pub(crate) history: Vec<Delivery>,
    /// The bounds in force.
    pub(crate) caps: Caps,
    /// What *this* call delivered: empty for a read, and the point of a tick.
    /// A tick that is late for five missed periods delivers once, so this is
    /// what to announce rather than the difference between two histories.
    pub(crate) fired: Vec<Delivery>,
}

/// The on-disk shape. Every field is defaulted so an older file (or a
/// hand-written one with a field missing) still loads.
#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    schedules: Vec<Schedule>,
    #[serde(default)]
    history: Vec<Delivery>,
    #[serde(default)]
    seq: u64,
}

/// The durable store behind the Schedule pane.
pub(crate) struct Store {
    path: PathBuf,
    file: File,
}

impl Store {
    /// Conventional store file: `~/.bos/gui/schedules.json`.
    pub(crate) fn default_path() -> PathBuf {
        PathBuf::from(shellexpand::tilde("~/.bos/gui/schedules.json").into_owned())
    }

    /// Load the conventional file.
    pub(crate) fn open() -> Self {
        Self::load(&Self::default_path())
    }

    /// Load from an explicit path (used by tests).
    ///
    /// A missing file is an empty store; a corrupt one is an empty store too —
    /// a reminder list must never keep the GUI from starting. Two things are
    /// sanitized on the way in: a repeat with a sub-minute step is dropped (it
    /// would otherwise fire on every tick) and the history is truncated to the
    /// cap, so a hand-edited file cannot make the store unbounded.
    pub(crate) fn load(path: &Path) -> Self {
        let mut file: File = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        file.version = FILE_VERSION;
        file.schedules.retain(|s| s.cadence.is_valid());
        file.history.truncate(MAX_HISTORY);
        Self {
            path: path.to_path_buf(),
            file,
        }
    }

    /// The reminders, in creation order.
    pub(crate) fn schedules(&self) -> &[Schedule] {
        &self.file.schedules
    }

    /// The deliveries, newest first.
    pub(crate) fn history(&self) -> &[Delivery] {
        &self.file.history
    }

    /// Add a reminder, or report why it cannot be added.
    pub(crate) fn add(
        &mut self,
        text: &str,
        cadence: Cadence,
        now_ms: u64,
    ) -> Result<Schedule, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a reminder needs some text".to_string());
        }
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!("a reminder is at most {MAX_TEXT_CHARS} characters"));
        }
        if let Cadence::Every { every_minutes, .. } = cadence {
            if every_minutes < MIN_EVERY_MINUTES {
                return Err(format!(
                    "a repeating reminder needs at least {MIN_EVERY_MINUTES} minute between deliveries"
                ));
            }
        }
        if !cadence.is_valid() {
            return Err("a reminder needs a time in the future or a valid repeat".to_string());
        }
        if self.file.schedules.len() >= MAX_SCHEDULES {
            return Err(format!(
                "at the cap of {MAX_SCHEDULES} reminders; cancel one first"
            ));
        }
        self.file.seq += 1;
        let schedule = Schedule {
            id: format!("sc-{now_ms}-{}", self.file.seq),
            text: text.to_string(),
            cadence,
            created_ms: now_ms,
            last_fired_ms: None,
            fired: 0,
        };
        self.file.schedules.push(schedule.clone());
        Ok(schedule)
    }

    /// Remove a reminder. Unknown ids are an error so the UI can say so.
    pub(crate) fn cancel(&mut self, id: &str) -> Result<(), String> {
        let before = self.file.schedules.len();
        self.file.schedules.retain(|s| s.id != id);
        if self.file.schedules.len() == before {
            return Err(format!("no reminder with id {id}"));
        }
        Ok(())
    }

    /// Forget every delivery, keeping the reminders.
    pub(crate) fn clear_history(&mut self) {
        self.file.history.clear();
    }

    /// Deliver everything due at `now_ms`, newest first, and advance the
    /// repeats.
    ///
    /// One delivery per reminder per tick, however long the gap: a repeat that
    /// missed five periods is delivered once and its next due lands in the
    /// future, because five notifications that all happened in the past are
    /// not five reminders. The advance is arithmetic rather than a loop, so a
    /// store that was closed for a month costs one step.
    pub(crate) fn tick(&mut self, now_ms: u64) -> Vec<Delivery> {
        let mut fired = Vec::new();
        let mut done = Vec::new();
        for (index, schedule) in self.file.schedules.iter_mut().enumerate() {
            let due = schedule.cadence.due_ms();
            if due > now_ms {
                continue;
            }
            fired.push(Delivery {
                schedule_id: schedule.id.clone(),
                text: schedule.text.clone(),
                at_ms: due,
                noticed_ms: now_ms,
            });
            schedule.last_fired_ms = Some(due);
            schedule.fired += 1;
            match &mut schedule.cadence {
                Cadence::Once { .. } => done.push(index),
                Cadence::Every {
                    every_minutes,
                    next_ms,
                } => {
                    let step = *every_minutes * 60_000;
                    let missed = now_ms.saturating_sub(*next_ms) / step + 1;
                    *next_ms += missed * step;
                }
            }
        }
        // Newest first: the caller shows one line per delivery, most recent on
        // top, and reversing a batch keeps a burst in due order.
        fired.reverse();
        for delivery in fired.iter().rev() {
            self.file.history.insert(0, delivery.clone());
        }
        self.file.history.truncate(MAX_HISTORY);
        // A one-shot has no next time, so it leaves the list; its delivery is
        // the record that it happened. Removing after the batch keeps the
        // indices above valid.
        for index in done.into_iter().rev() {
            self.file.schedules.remove(index);
        }
        fired
    }

    /// Everything the pane draws, with nothing counted as just delivered.
    pub(crate) fn report(&self, now_ms: u64) -> Report {
        self.report_with(now_ms, Vec::new())
    }

    /// Everything the pane draws, plus the deliveries this call made.
    pub(crate) fn report_with(&self, now_ms: u64, fired: Vec<Delivery>) -> Report {
        Report {
            now_ms,
            schedules: self.schedules().to_vec(),
            history: self.history().to_vec(),
            caps: Caps {
                min_every_minutes: MIN_EVERY_MINUTES,
                max_schedules: MAX_SCHEDULES,
                max_history: MAX_HISTORY,
                max_text_chars: MAX_TEXT_CHARS,
            },
            fired,
        }
    }

    /// Write the store, replacing the file atomically: a reminder list that is
    /// half-written after a crash is a list that lost reminders.
    pub(crate) fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&self.file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store on a fresh temp path, with the file removed first.
    fn store(name: &str) -> Store {
        let path = std::env::temp_dir().join(format!("bos-sched-{name}.json"));
        let _ = fs::remove_file(&path);
        Store::load(&path)
    }

    const MIN: u64 = 60_000;

    #[test]
    fn a_once_reminder_fires_then_leaves_the_list() {
        let mut s = store("once");
        s.add("stand up", Cadence::Once { at_ms: 5 * MIN }, 0)
            .unwrap();
        assert!(s.tick(4 * MIN).is_empty(), "not due yet");
        assert_eq!(s.schedules().len(), 1);
        let fired = s.tick(5 * MIN);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].text, "stand up");
        assert_eq!(fired[0].at_ms, 5 * MIN);
        assert!(s.schedules().is_empty(), "a one-shot has no next time");
        assert_eq!(s.history().len(), 1, "its delivery is the record");
        assert_eq!(s.history()[0].noticed_ms, 5 * MIN, "delivered on time");
        assert_eq!(s.history()[0].schedule_id, fired[0].schedule_id);
    }

    #[test]
    fn a_repeat_forgets_the_periods_it_missed() {
        let mut s = store("repeat");
        s.add(
            "breathe",
            Cadence::Every {
                every_minutes: 5,
                next_ms: 5 * MIN,
            },
            0,
        )
        .unwrap();
        // The window was closed for an hour: four missed periods, one delivery.
        let fired = s.tick(65 * MIN);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].at_ms, 5 * MIN, "recorded when it was due");
        assert_eq!(fired[0].noticed_ms, 65 * MIN, "and when it was noticed");
        assert_eq!(
            s.history()[0].noticed_ms,
            65 * MIN,
            "a late delivery keeps both times, so the pane can say how late"
        );
        let again = s.tick(65 * MIN);
        assert!(
            again.is_empty(),
            "one delivery per tick, not one per period"
        );
        assert_eq!(s.schedules().len(), 1, "a repeat stays");
        let sched = &s.schedules()[0];
        assert_eq!(sched.fired, 1);
        assert_eq!(sched.last_fired_ms, Some(5 * MIN));
        assert_eq!(
            sched.cadence.due_ms(),
            70 * MIN,
            "the next due is strictly after now, not a backlog"
        );
        assert_eq!(s.tick(70 * MIN).len(), 1, "and it fires again on time");
    }

    #[test]
    fn a_repeat_needs_at_least_a_minute() {
        let mut s = store("subminute");
        let err = s
            .add(
                "too eager",
                Cadence::Every {
                    every_minutes: 0,
                    next_ms: MIN,
                },
                0,
            )
            .expect_err("sub-minute repeats are refused");
        assert!(err.contains("at least 1 minute"), "{err}");
        assert!(s.schedules().is_empty());
    }

    #[test]
    fn the_text_is_trimmed_and_bounded() {
        let mut s = store("text");
        s.add("  water the plants  ", Cadence::Once { at_ms: MIN }, 0)
            .unwrap();
        assert_eq!(s.schedules()[0].text, "water the plants");
        assert!(
            s.add("   ", Cadence::Once { at_ms: MIN }, 0).is_err(),
            "empty"
        );
        let long = "x".repeat(MAX_TEXT_CHARS + 1);
        let err = s.add(&long, Cadence::Once { at_ms: MIN }, 0).unwrap_err();
        assert!(err.contains(&MAX_TEXT_CHARS.to_string()), "{err}");
        let exact = "y".repeat(MAX_TEXT_CHARS);
        assert!(s.add(&exact, Cadence::Once { at_ms: MIN }, 0).is_ok());
    }

    #[test]
    fn the_store_refuses_reminder_past_its_cap() {
        let mut s = store("cap");
        for i in 0..MAX_SCHEDULES {
            s.add(&format!("r{i}"), Cadence::Once { at_ms: MIN }, 0)
                .unwrap();
        }
        let err = s
            .add("one too many", Cadence::Once { at_ms: MIN }, 0)
            .unwrap_err();
        assert!(err.contains(&MAX_SCHEDULES.to_string()), "{err}");
        assert_eq!(s.schedules().len(), MAX_SCHEDULES);
    }

    #[test]
    fn history_keeps_the_newest_two_hundred() {
        let mut s = store("history");
        // One reminder that fires every minute, ticked well past the cap.
        s.add(
            "tick",
            Cadence::Every {
                every_minutes: 1,
                next_ms: MIN,
            },
            0,
        )
        .unwrap();
        for i in 1..=(MAX_HISTORY as u64 + 5) {
            let fired = s.tick(i * MIN);
            assert_eq!(fired.len(), 1, "tick {i}");
        }
        assert_eq!(s.history().len(), MAX_HISTORY);
        assert_eq!(s.history()[0].at_ms, (MAX_HISTORY as u64 + 5) * MIN);
        assert_eq!(
            s.history()[MAX_HISTORY - 1].at_ms,
            6 * MIN,
            "the oldest five fell off"
        );
    }

    #[test]
    fn a_reminder_survives_a_restart() {
        let mut s = store("restart");
        s.add("ship it", Cadence::Once { at_ms: 9 * MIN }, 3 * MIN)
            .unwrap();
        s.add(
            "hourly",
            Cadence::Every {
                every_minutes: 60,
                next_ms: 60 * MIN,
            },
            3 * MIN,
        )
        .unwrap();
        s.save().unwrap();
        let mut again = Store::load(&s.path);
        assert_eq!(again.schedules().len(), 2);
        assert_eq!(again.schedules()[0].text, "ship it");
        assert_eq!(again.schedules()[0].created_ms, 3 * MIN);
        assert_eq!(
            again.schedules()[1].cadence,
            Cadence::Every {
                every_minutes: 60,
                next_ms: 60 * MIN
            }
        );
        assert_eq!(again.schedules()[1].id, s.schedules()[1].id);
        // The reloaded store still fires.
        assert_eq!(again.tick(9 * MIN).len(), 1);
    }

    #[test]
    fn a_missing_or_corrupt_file_is_an_empty_store() {
        let path = std::env::temp_dir().join("bos-sched-corrupt.json");
        let _ = fs::remove_file(&path);
        assert!(Store::load(&path).schedules().is_empty(), "missing");
        fs::write(&path, "{ not json at all").unwrap();
        assert!(
            Store::load(&path).schedules().is_empty(),
            "a corrupt file must not keep the GUI from starting"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_hand_edited_sub_minute_repeat_is_dropped_on_load() {
        let path = std::env::temp_dir().join("bos-sched-edited.json");
        fs::write(
            &path,
            r#"{"schedules":[
                 {"id":"a","text":"spin","cadence":{"kind":"every","every_minutes":0,"next_ms":60000},
                  "created_ms":0,"last_fired_ms":null,"fired":0},
                 {"id":"b","text":"keep","cadence":{"kind":"every","every_minutes":2,"next_ms":60000},
                  "created_ms":0,"last_fired_ms":null,"fired":0}],
               "history":[]}"#,
        )
        .unwrap();
        let s = Store::load(&path);
        assert_eq!(s.schedules().len(), 1, "only the valid repeat survives");
        assert_eq!(s.schedules()[0].id, "b");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_report_carries_the_clock_and_the_bounds() {
        let mut s = store("report");
        s.add("one", Cadence::Once { at_ms: 2 * MIN }, 0).unwrap();
        let report = s.report(MIN);
        assert_eq!(report.now_ms, MIN);
        assert_eq!(report.schedules.len(), 1);
        assert_eq!(report.caps.min_every_minutes, MIN_EVERY_MINUTES);
        assert_eq!(report.caps.max_schedules, MAX_SCHEDULES);
        assert_eq!(report.caps.max_history, MAX_HISTORY);
        assert_eq!(report.caps.max_text_chars, MAX_TEXT_CHARS);
        // The report is what the UI parses, so it must round-trip as JSON with
        // the field names the JS reads.
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["caps"]["min_every_minutes"], 1);
        assert_eq!(json["schedules"][0]["cadence"]["kind"], "once");
        assert_eq!(json["schedules"][0]["cadence"]["at_ms"], 2 * MIN);
        assert!(json["history"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_report_says_what_this_call_delivered() {
        let mut s = store("fired");
        s.add("ping", Cadence::Once { at_ms: MIN }, 0).unwrap();
        // A read reports nothing as delivered, even with a reminder due: only a
        // tick delivers, and only its own deliveries are announced.
        assert!(s.report(MIN).fired.is_empty());
        assert_eq!(s.schedules().len(), 1, "a read does not fire");
        let fired = s.tick(MIN);
        let report = s.report_with(MIN, fired);
        assert_eq!(report.fired.len(), 1);
        assert_eq!(report.fired[0].text, "ping");
        assert_eq!(report.fired[0].noticed_ms, MIN);
        assert_eq!(report.history.len(), 1);
        assert!(
            s.report(MIN).fired.is_empty(),
            "and a later read is quiet again"
        );
    }

    #[test]
    fn cancelling_an_unknown_reminder_says_so() {
        let mut s = store("cancel");
        s.add("keep", Cadence::Once { at_ms: MIN }, 0).unwrap();
        let err = s.cancel("sc-nope-1").unwrap_err();
        assert!(err.contains("sc-nope-1"), "{err}");
        assert_eq!(s.schedules().len(), 1, "and nothing was removed");
        s.cancel(&s.schedules()[0].id.clone()).unwrap();
        assert!(s.schedules().is_empty());
    }

    #[test]
    fn clearing_history_keeps_the_reminders() {
        let mut s = store("clear");
        s.add("ping", Cadence::Once { at_ms: MIN }, 0).unwrap();
        s.add("keep", Cadence::Once { at_ms: 9 * MIN }, 0).unwrap();
        s.tick(MIN);
        assert_eq!(s.history().len(), 1);
        assert_eq!(s.schedules().len(), 1, "the one-shot left");
        s.clear_history();
        assert!(s.history().is_empty());
        assert_eq!(s.schedules().len(), 1, "clearing history is not cancelling");
    }

    #[test]
    fn ids_are_unique_inside_one_millisecond() {
        let mut s = store("ids");
        let a = s.add("a", Cadence::Once { at_ms: MIN }, 7).unwrap();
        let b = s.add("b", Cadence::Once { at_ms: MIN }, 7).unwrap();
        assert_ne!(a.id, b.id, "a burst of adds must not collide");
        s.cancel(&a.id).unwrap();
        assert_eq!(s.schedules().len(), 1);
        assert_eq!(s.schedules()[0].id, b.id);
    }
}
