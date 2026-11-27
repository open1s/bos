//! The inspector column: the mock's third column.
//!
//! The concept draws a rail of sessions, the conversation, and then a column of
//! facts about what the workspace is doing: the runtime's health, the jobs it is
//! running, the goal behind them and the last few events that landed. Every one
//! of those is something the client already holds - the host's status, the open
//! projection, the session list - so this module is a painting of state the app
//! gathered, not a source of its own.
//!
//! It is a plain data structure in and rows out, so the layout can be checked
//! without a terminal: [`lines`] answers exactly the rows a frame of a given
//! width and height would paint.

use chrono::{DateTime, Local, Utc};

use super::app::{cells, truncate};
use super::skin::Tone;

/// The narrowest frame that carries the inspector.
///
/// Below it the concept drops the column rather than squeezing three panes onto
/// a terminal too narrow to read them, which is what
/// `docs/tui/requirements.md` calls the two-column layout.
pub(crate) const MIN_FRAME: usize = 120;

/// The inspector's width on a frame `frame` cells wide, or `None` when the
/// frame is too narrow to carry one.
///
/// A fifth of the frame, bounded by what is worth reading: the mock's own
/// column is a fifth of its canvas, and a terminal smaller than that canvas
/// would otherwise let the column eat the conversation.
pub(crate) fn width(frame: usize) -> Option<usize> {
    (frame >= MIN_FRAME).then(|| (frame / 5).clamp(22, 34))
}

/// One row of runs: a piece of text and the ink it is painted in.
pub(crate) type Row = Vec<(String, Tone)>;

/// A call the open session is running right now.
pub(crate) struct Active {
    /// The tool the model called.
    pub(crate) tool: String,
    /// When the call started, so the column can say how long it has run.
    pub(crate) started: DateTime<Utc>,
    /// Whether the call is waiting on the reader's approval rather than
    /// running: a turn blocked on a question is the fact a reader most needs
    /// out of this column.
    pub(crate) waiting: bool,
}

/// One event the column remembers, as the last few that landed.
pub(crate) struct Recent {
    /// When it landed.
    pub(crate) at: DateTime<Utc>,
    /// What happened, in a few words.
    pub(crate) what: String,
    /// The ink the verdict earns.
    pub(crate) tone: Tone,
}

/// The goal the open session pursues, when it has one.
pub(crate) struct Goal {
    /// What the work is for.
    pub(crate) objective: String,
    /// Rounds spent.
    pub(crate) rounds: u32,
    /// Rounds allowed.
    pub(crate) max_rounds: u32,
}

/// One agent behind the open session's work.
///
/// The concept's column names the agents and their states; a terminal has the
/// Host's own delegation roster, so the row carries the label the model gave the
/// child and the lifecycle word the record holds - never a guess that a listed
/// agent is executing.
pub(crate) struct Agent {
    /// The short description the model supplied, or the run's id when it gave
    /// none.
    pub(crate) name: String,
    /// The record's own lifecycle word: `running`, `done` or `failed`.
    pub(crate) status: String,
    /// How deep below the conversation the run sits: one is a direct child.
    pub(crate) depth: usize,
}

/// A durable workspace job the Host listed as active.
pub(crate) struct Job {
    /// The stable identifier, shortened only for the narrow summary column.
    pub(crate) id: String,
    /// The operator-facing task name.
    pub(crate) title: String,
    /// The Host's lifecycle state.
    pub(crate) status: String,
    /// The attempt count the task record carries.
    pub(crate) attempts: u32,
}

/// Everything the inspector paints, gathered by the app.
pub(crate) struct Board {
    /// The host's verdict, in one word: `Healthy`, `Starting`, `Degraded`.
    pub(crate) host: String,
    /// The ink that verdict earns.
    pub(crate) host_tone: Tone,
    /// Sessions in the workspace.
    pub(crate) sessions: usize,
    /// Turns running across the workspace, the open one included.
    pub(crate) running: usize,
    /// How long the Host has been up.
    pub(crate) uptime: String,
    /// Calls the open session is running.
    pub(crate) active: Vec<Active>,
    /// Durable tasks that are running or queued workspace-wide.
    pub(crate) jobs: Vec<Job>,
    /// Whether the Host answered the task-board query.
    pub(crate) jobs_loaded: bool,
    /// The agents behind the open session's work, newest first.
    pub(crate) agents: Vec<Agent>,
    /// The open session's goal.
    pub(crate) goal: Option<Goal>,
    /// The last few events, newest first.
    pub(crate) recent: Vec<Recent>,
    /// The instant the frame is painted, so elapsed times are the reader's.
    pub(crate) now: DateTime<Utc>,
}

/// The rows the inspector paints on a column `width` cells wide and `rows` tall.
///
/// Exactly `rows` rows come back - a short board is padded with blanks - because
/// the frame composes columns row by row and a ragged column would shift the
/// speaker on the row above it.
pub(crate) fn lines(board: &Board, width: usize, rows: usize) -> Vec<Row> {
    // The panel's own padding: the mock keeps a cell of air on either side of
    // the words in this column, and a hairline that runs edge to edge.
    let inner = width.saturating_sub(2);
    let mut painted: Vec<Row> = Vec::new();

    // System status: what the runtime is and whether it is well.
    painted.push(title("SYSTEM STATUS"));
    painted.push(pair("Runtime Host", &board.host, board.host_tone, inner));
    painted.push(pair("Runtime", "Ratatui", Tone::Body, inner));
    painted.push(pair("Uptime", &board.uptime, Tone::Body, inner));
    painted.push(pair(
        "Sessions",
        &board.sessions.to_string(),
        Tone::Body,
        inner,
    ));
    // The roster's size, not its liveness: an agent the Host lists is one that
    // can be assigned work, which is not the same fact as one executing now.
    painted.push(pair(
        "Agents",
        &match board.agents.len() {
            0 => "none".to_string(),
            1 => "1 run".to_string(),
            n => format!("{n} runs"),
        },
        match board.agents.is_empty() {
            true => Tone::Muted,
            false => Tone::Body,
        },
        inner,
    ));
    painted.push(pair(
        "Jobs",
        &match board.running {
            0 => "idle".to_string(),
            1 => "1 running".to_string(),
            n => format!("{n} running"),
        },
        match board.running {
            0 => Tone::Muted,
            _ => Tone::Warn,
        },
        inner,
    ));

    // Active jobs: the calls the reader can watch happen.
    painted.push(blank());
    painted.push(rule(width));
    painted.push(title("ACTIVE CALLS"));
    if board.active.is_empty() {
        painted.push(vec![("Nothing running".to_string(), Tone::Muted)]);
    }
    for active in board.active.iter().take(3) {
        // The dot carries the state and the value is the time spent, because
        // that is what a reader waiting on a call wants to know; a call held on
        // the reader's answer says the word instead, since that is the fact.
        let spent = elapsed(board.now, active.started);
        let (glyph, value, tone) = match active.waiting {
            true => ("?", "waiting".to_string(), Tone::Warn),
            false => ("●", spent, Tone::Warn),
        };
        painted.push(pair(
            &format!("{glyph} {}", active.tool),
            &value,
            tone,
            inner,
        ));
    }

    // Durable task records are workspace-wide and outlive the conversation
    // that started them, so the summary comes from the Host's task board. Keep
    // this preview on a tall frame; short terminals give those rows back to the
    // goal and recent activity summaries.
    if rows >= 28 {
        painted.push(blank());
        painted.push(rule(width));
        painted.push(title_action("ACTIVE JOBS", width));
        if !board.jobs_loaded {
            painted.push(vec![(
                "Jobs unavailable · open Jobs to retry".to_string(),
                Tone::Muted,
            )]);
        } else if board.jobs.is_empty() {
            painted.push(vec![("No active jobs".to_string(), Tone::Muted)]);
        }
        for job in board.jobs.iter().take(2) {
            painted.push(pair(
                &format!("#{} {}", job.id, job.title),
                &job.status,
                if job.status == "running" {
                    Tone::Warn
                } else {
                    Tone::Muted
                },
                inner,
            ));
            if job.attempts > 0 {
                painted.push(vec![(
                    format!("  {} attempt(s)", job.attempts),
                    Tone::Muted,
                )]);
            }
        }
    }

    // Agents: who is behind the work, as the concept's column lists them.
    // A row's dot carries the state the Host reported, so a listed agent that
    // has not started yet cannot read as one that is executing.
    painted.push(blank());
    painted.push(rule(width));
    painted.push(title_action("SESSION AGENTS", width));
    if board.agents.is_empty() {
        painted.push(vec![("No agent yet".to_string(), Tone::Muted)]);
    }
    for agent in board.agents.iter().take(3) {
        let (glyph, tone) = match agent.status.as_str() {
            "running" => ("●", Tone::Warn),
            "done" => ("✓", Tone::Ok),
            "failed" => ("!", Tone::Error),
            _ => ("·", Tone::Muted),
        };
        // A nested run says how deep it sits: a child of a child is a fact a
        // reader supervising a delegation needs to see without opening it.
        let nest = if agent.depth > 1 {
            format!("{}└ ", "  ".repeat(agent.depth - 1))
        } else {
            String::new()
        };
        painted.push(pair(
            &format!("{glyph} {nest}{}", agent.name),
            &agent.status,
            tone,
            inner,
        ));
    }

    // The goal, when there is one: rounds are the one honest progress bar this
    // client can draw, so the bar is drawn from them and nothing else.
    if let Some(goal) = board.goal.as_ref() {
        painted.push(blank());
        painted.push(rule(width));
        painted.push(title("GOAL"));
        painted.push(vec![(truncate(&goal.objective, inner), Tone::Body)]);
        painted.push(progress(goal.rounds, goal.max_rounds, inner));
    }

    // Recent activity: the last few events, newest first.
    painted.push(blank());
    painted.push(rule(width));
    painted.push(title_action("SESSION ACTIVITY", width));
    if board.recent.is_empty() {
        painted.push(vec![("No activity yet".to_string(), Tone::Muted)]);
    }
    for recent in board.recent.iter().take(4) {
        painted.push(vec![
            (
                recent.at.with_timezone(&Local).format("%H:%M").to_string(),
                Tone::Muted,
            ),
            ("  ".to_string(), Tone::Muted),
            (truncate(&recent.what, inner.saturating_sub(7)), recent.tone),
        ]);
    }

    painted.resize(rows, Vec::new());
    painted.truncate(rows);
    // A row over the column's width would push the frame's right edge, so each
    // one is set inside the panel's padding and cut to what the column holds.
    painted.into_iter().map(|runs| inset(runs, width)).collect()
}

/// How long `started` has run by `now`, in the shortest unit that is honest.
fn elapsed(now: DateTime<Utc>, started: DateTime<Utc>) -> String {
    let seconds = (now - started).num_seconds().max(0) as u64;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        _ => format!("{}h", seconds / 3600),
    }
}

/// A section's name: the mock sets these in the muted ink, uppercase.
fn title(word: &str) -> Row {
    vec![(word.to_string(), Tone::Muted)]
}

/// A section heading with the reference design's keyboard-accessible
/// destination cue when the column is wide enough to keep both labels legible.
fn title_action(word: &str, width: usize) -> Row {
    let action = "View All →";
    let room = width.saturating_sub(2);
    let needed = cells(word) + cells(action) + 1;
    if room < needed {
        return title(word);
    }
    vec![
        (word.to_string(), Tone::Muted),
        (" ".repeat(room - cells(word) - cells(action)), Tone::Body),
        (action.to_string(), Tone::Accent),
    ]
}

/// The hairline that separates the column's sections.
fn rule(width: usize) -> Row {
    vec![("─".repeat(width), Tone::Muted)]
}

/// The blank row that gives the sections room to breathe.
fn blank() -> Row {
    Vec::new()
}

/// A fact: the mute label on the left, the value against the right edge.
fn pair(label: &str, value: &str, tone: Tone, width: usize) -> Row {
    let room = width.saturating_sub(1);
    let value = truncate(value, room.saturating_sub(2).max(1));
    let label = truncate(label, room.saturating_sub(cells(&value) + 1).max(1));
    let gap = room.saturating_sub(cells(&label) + cells(&value));
    vec![
        (label, Tone::Muted),
        (" ".repeat(gap), Tone::Muted),
        (value, tone),
    ]
}

/// Rounds spent out of the cap, as the bar the mock draws under a job.
fn progress(done: u32, max: u32, width: usize) -> Row {
    let max = max.max(1);
    let done = done.min(max);
    let caption = format!("{done}/{max} rounds");
    let cells_wide = width.saturating_sub(cells(&caption) + 3).clamp(3, 12);
    let filled = ((done as usize) * cells_wide + (max as usize / 2)) / max as usize;
    let filled = filled.min(cells_wide);
    vec![
        ("█".repeat(filled), Tone::Accent),
        ("░".repeat(cells_wide - filled), Tone::Muted),
        ("  ".to_string(), Tone::Muted),
        (caption, Tone::Muted),
    ]
}

/// A painted row inside the panel's padding: one cell of air on the left, the
/// runs, and the column filled to its width so the rule beside it keeps its
/// place. A hairline is the one row that runs edge to edge.
fn inset(runs: Row, width: usize) -> Row {
    let is_rule = !runs.is_empty()
        && runs
            .iter()
            .all(|(text, _)| !text.is_empty() && text.chars().all(|character| character == '─'));
    if is_rule {
        return clip(runs, width);
    }
    let mut row = clip(runs, width.saturating_sub(2));
    let used: usize = row.iter().map(|(text, _)| cells(text)).sum();
    let fill = width.saturating_sub(used + 1);
    let mut inset: Row = vec![(" ".to_string(), Tone::Body)];
    inset.append(&mut row);
    if fill > 0 {
        inset.push((" ".repeat(fill), Tone::Body));
    }
    inset
}

/// Cut a row's runs to `width` cells, dropping what does not fit.
fn clip(runs: Row, width: usize) -> Row {
    let mut kept: Row = Vec::new();
    let mut used = 0usize;
    for (text, tone) in runs {
        let advance = cells(&text);
        if used + advance <= width {
            used += advance;
            kept.push((text, tone));
            continue;
        }
        let room = width.saturating_sub(used);
        if room > 0 {
            kept.push((truncate(&text, room), tone));
        }
        break;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 4, hour, minute, 0).unwrap()
    }

    fn text(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|runs| {
                runs.iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .trim_start()
                    .to_string()
            })
            .collect()
    }

    fn board() -> Board {
        Board {
            host: "Healthy".to_string(),
            host_tone: Tone::Ok,
            sessions: 3,
            running: 1,
            uptime: "2h 14m".to_string(),
            active: vec![Active {
                tool: "shell.run".to_string(),
                started: at(14, 30),
                waiting: false,
            }],
            jobs: Vec::new(),
            jobs_loaded: true,
            goal: None,
            agents: Vec::new(),
            recent: vec![Recent {
                at: at(14, 32),
                what: "shell.run done".to_string(),
                tone: Tone::Ok,
            }],
            now: at(14, 30),
        }
    }

    #[test]
    fn the_column_appears_only_when_the_frame_has_room() {
        assert_eq!(width(80), None);
        assert_eq!(width(119), None);
        assert_eq!(width(120), Some(24));
        assert_eq!(width(160), Some(32));
        // A very wide terminal does not let the column take over: it is a
        // summary, and the conversation keeps the rest.
        assert_eq!(width(400), Some(34));
    }

    #[test]
    fn the_board_names_the_runtime_and_the_jobs_it_runs() {
        let rows = lines(&board(), 24, 20);
        let painted = text(&rows);
        assert!(painted.iter().any(|row| row.starts_with("SYSTEM STATUS")));
        assert!(painted.iter().any(|row| row.contains("Runtime Host")));
        assert!(painted.iter().any(|row| row.contains("Healthy")));
        assert!(painted.iter().any(|row| row.contains("Ratatui")));
        assert!(painted.iter().any(|row| row.contains("2h 14m")));
        assert!(painted.iter().any(|row| row.contains("3")));
        assert!(painted.iter().any(|row| row.contains("1 running")));
        assert!(painted.iter().any(|row| row == "ACTIVE CALLS"));
        assert!(painted.iter().any(|row| row == "SESSION AGENTS"));
        assert!(painted.iter().any(|row| row == "SESSION ACTIVITY"));
    }

    #[test]
    fn the_tall_board_previews_host_jobs_with_their_real_state() {
        let mut board = board();
        board.jobs.push(Job {
            id: "1234abcd".to_string(),
            title: "Build".to_string(),
            status: "running".to_string(),
            attempts: 2,
        });
        let painted = text(&lines(&board, 34, 40));
        assert!(painted.iter().any(|row| row.contains("ACTIVE JOBS")));
        assert!(painted.iter().any(|row| row.contains("View All")));
        assert!(painted.iter().any(|row| row.contains("#1234abcd Build")));
        assert!(painted.iter().any(|row| row.contains("2 attempt(s)")));
    }

    #[test]
    fn a_running_call_is_listed_with_the_time_it_has_taken() {
        let mut board = board();
        board.active[0].started = at(14, 29);
        let painted = text(&lines(&board, 24, 20));
        let row = painted
            .iter()
            .find(|row| row.contains("shell.run"))
            .expect("the running call is listed");
        assert!(row.starts_with('●'), "a running call is marked: {row:?}");
        assert!(row.contains("1m"), "elapsed time is painted: {row:?}");
    }

    #[test]
    fn a_call_waiting_on_the_reader_says_so() {
        let mut board = board();
        board.active[0].waiting = true;
        let painted = text(&lines(&board, 24, 20));
        let row = painted
            .iter()
            .find(|row| row.contains("shell.run"))
            .unwrap();
        assert!(row.starts_with('?'), "waiting is marked: {row:?}");
        assert!(row.contains("waiting"));
    }

    #[test]
    fn an_idle_workspace_says_it_is_idle_and_running_nothing() {
        let mut board = board();
        board.running = 0;
        board.active.clear();
        let painted = text(&lines(&board, 24, 20));
        assert!(painted.iter().any(|row| row.contains("idle")));
        assert!(painted.iter().any(|row| row.contains("Nothing running")));
    }

    #[test]
    fn a_goal_is_painted_as_the_rounds_it_has_spent() {
        let mut board = board();
        board.goal = Some(Goal {
            objective: "port the rail".to_string(),
            rounds: 4,
            max_rounds: 8,
        });
        let painted = text(&lines(&board, 24, 20));
        assert!(painted.iter().any(|row| row.starts_with("GOAL")));
        assert!(painted.iter().any(|row| row.contains("port the rail")));
        let bar = painted
            .iter()
            .find(|row| row.contains("4/8 rounds"))
            .expect("the rounds are painted");
        assert!(bar.contains('█'), "the bar is drawn: {bar:?}");
        assert!(bar.contains('░'));
    }

    #[test]
    fn recent_activity_is_the_newest_event_first() {
        let mut board = board();
        board.recent = vec![
            Recent {
                at: at(14, 35),
                what: "you spoke".to_string(),
                tone: Tone::User,
            },
            Recent {
                at: at(14, 32),
                what: "call done".to_string(),
                tone: Tone::Ok,
            },
        ];
        let painted = text(&lines(&board, 24, 24));
        let newest = painted
            .iter()
            .position(|row| row.contains("you spoke"))
            .unwrap();
        let older = painted
            .iter()
            .position(|row| row.contains("call done"))
            .unwrap();
        assert!(newest < older, "newest first: {painted:?}");
    }

    #[test]
    fn every_row_stays_inside_the_column() {
        let mut board = board();
        board.goal = Some(Goal {
            objective: "a very long objective that will not fit in the column at all".to_string(),
            rounds: 12,
            max_rounds: 16,
        });
        board.recent = vec![Recent {
            at: at(14, 35),
            what: "some long event that also would not fit the column".to_string(),
            tone: Tone::Body,
        }];
        for width in [22usize, 24, 30, 34] {
            for row in lines(&board, width, 24) {
                let used: usize = row.iter().map(|(text, _)| cells(text)).sum();
                assert!(used <= width, "row over {width} cells: {row:?}");
            }
        }
    }

    #[test]
    fn the_roster_says_how_many_agents_can_take_work() {
        let mut board = board();
        assert!(
            text(&lines(&board, 24, 20))
                .iter()
                .any(|row| row.contains("none"))
        );
        board.agents = vec![
            Agent {
                name: "probe".to_string(),
                status: "running".to_string(),
                depth: 1,
            },
            Agent {
                name: "port the rail".to_string(),
                status: "done".to_string(),
                depth: 2,
            },
        ];
        let painted = text(&lines(&board, 24, 22));
        assert!(painted.iter().any(|row| row.contains("2 runs")));
    }

    #[test]
    fn an_agent_is_marked_by_its_status_and_nested_by_its_depth() {
        let mut board = board();
        board.agents = vec![
            Agent {
                name: "port the rail".to_string(),
                status: "running".to_string(),
                depth: 1,
            },
            Agent {
                name: "child".to_string(),
                status: "done".to_string(),
                depth: 2,
            },
            Agent {
                name: "other".to_string(),
                status: "failed".to_string(),
                depth: 1,
            },
        ];
        let painted = text(&lines(&board, 28, 26));
        let running = painted
            .iter()
            .find(|row| row.contains("port the rail"))
            .expect("the running agent is listed");
        assert!(running.contains('●'), "running is marked: {running:?}");
        let child = painted
            .iter()
            .find(|row| row.contains("child"))
            .expect("the child is listed");
        assert!(child.contains('└'), "a child sits deeper: {child:?}");
        assert!(child.contains('✓'), "done is marked: {child:?}");
        let failed = painted
            .iter()
            .find(|row| row.contains("other"))
            .expect("the failed agent is listed");
        assert!(failed.contains('!'), "failed is marked: {failed:?}");
    }

    #[test]
    fn a_short_frame_still_paints_exactly_its_rows() {
        for rows in [0usize, 1, 6, 19, 40] {
            assert_eq!(lines(&board(), 24, rows).len(), rows);
        }
    }
}
