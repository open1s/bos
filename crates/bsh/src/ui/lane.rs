//! The lane strip's geometry.
//!
//! The trajectory lanes (Input / Model / Tools) share ONE time axis, built from
//! the union of every lane's events, so a block's left edge is the instant its
//! event started and its width is how long it took. Both are fractions of the
//! lane width, which is what lets the renderer place a block without knowing the
//! measured track width at layout time; the tests multiply the fractions by the
//! measured track to pin the pixel geometry.
//!
//! A block is never dropped: an event without a start is placed at the nearest
//! known instant in its lane, and an event without a duration keeps the
//! renderer's two-pixel floor. Those two fallbacks are the only cases where the
//! geometry is an approximation, and they are marked where they are applied.

/// One event as the lane strip needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LaneRecord {
    /// The event's index in the trajectory log.
    pub(crate) index: usize,
    /// The turn the event belongs to.
    pub(crate) turn: usize,
    /// When the event started, epoch milliseconds, when the record carries it.
    pub(crate) start: Option<u64>,
    /// How long the event took, when a clock measured it.
    pub(crate) duration_ms: Option<u64>,
}

/// The time domain every lane is drawn against.
///
/// Built from the union of all lanes, never from one lane alone: that is what
/// makes two events that started together line up vertically across lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimeAxis {
    /// The earliest recorded start across every lane.
    pub(crate) start: u64,
    /// The latest recorded end across every lane.
    pub(crate) end: u64,
}

impl TimeAxis {
    /// The union of every lane's clock, or `None` when no event has a start.
    ///
    /// `None` is not an empty strip: the caller falls back to record order, so a
    /// log recorded before timestamps existed still draws every event.
    pub(crate) fn across(lanes: &[&[LaneRecord]]) -> Option<Self> {
        let mut start: Option<u64> = None;
        let mut end: Option<u64> = None;
        for lane in lanes {
            for record in *lane {
                let Some(at) = record.start else {
                    continue;
                };
                start = Some(start.map_or(at, |current| current.min(at)));
                let finish = at.saturating_add(record.duration_ms.unwrap_or(0));
                end = Some(end.map_or(finish, |current| current.max(finish)));
            }
        }
        start.zip(end).map(|(start, end)| Self {
            start,
            end: end.max(start),
        })
    }

    /// The axis length in milliseconds.
    ///
    /// Floored at one so a run where every event shares an instant still has a
    /// divisor; every block then sits at zero and the renderer's minimum width
    /// keeps it visible.
    pub(crate) fn span_ms(self) -> u64 {
        self.end.saturating_sub(self.start).max(1)
    }

    /// Where `at` falls on the axis, as a fraction of the lane width.
    pub(crate) fn x(self, at: u64) -> f32 {
        (at.saturating_sub(self.start) as f64 / self.span_ms() as f64) as f32
    }

    /// How wide `ms` is on the axis, as a fraction of the lane width.
    pub(crate) fn width(self, ms: u64) -> f32 {
        (ms as f64 / self.span_ms() as f64) as f32
    }
}

/// One block placed on a lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LaneBlock {
    /// The event's index in the trajectory log.
    pub(crate) index: usize,
    /// The turn the event belongs to.
    pub(crate) turn: usize,
    /// The block's left edge, as a fraction of the lane width.
    pub(crate) x: f32,
    /// The block's width, as a fraction of the lane width.
    pub(crate) width: f32,
}

/// Place one lane on the shared axis.
///
/// Every record produces exactly one block, in log order, so the caller can
/// pair each block with the row it stands for. A missing START is placed at the
/// nearest known instant in the lane - the previous timed event, else the next
/// one, else the axis start - because dropping the row would hide a real event.
/// A missing DURATION contributes zero width here; the renderer raises that to
/// its two-pixel floor so the block stays a control.
pub(crate) fn time_lane(lane: &[LaneRecord], axis: TimeAxis) -> Vec<LaneBlock> {
    // The first timed event at or after each slot, so an untimed record with no
    // preceding clock still lands beside its neighbours rather than at zero.
    let mut next_known: Vec<Option<u64>> = vec![None; lane.len()];
    let mut upcoming: Option<u64> = None;
    for slot in (0..lane.len()).rev() {
        if lane[slot].start.is_some() {
            upcoming = lane[slot].start;
        }
        next_known[slot] = upcoming;
    }
    let mut carry: Option<u64> = None;
    lane.iter()
        .enumerate()
        .map(|(slot, record)| {
            let at = match record.start {
                Some(at) => {
                    carry = Some(at);
                    at
                }
                // Fallback for a record written before the clock existed: the
                // nearest known time in this lane, then the shared axis start.
                None => carry.or(next_known[slot]).unwrap_or(axis.start),
            };
            let ms = record.duration_ms.unwrap_or(0);
            let x = axis.x(at);
            let width = axis.width(ms);
            LaneBlock {
                index: record.index,
                turn: record.turn,
                x,
                // An event with no start was placed by approximation, so its
                // duration can reach past the axis end. Clamping the WIDTH (never
                // the left edge) keeps it inside the lane without moving the
                // position the fallback chose.
                width: if x + width > 1.0 {
                    (1.0 - x).max(0.0)
                } else {
                    width
                },
            }
        })
        .collect()
}

/// Equal shares, one slot per record, for the modes that are not a clock.
///
/// This is the fallback for a Duration strip whose session carries no start
/// timestamps at all: every event still draws, in record order, instead of the
/// strip going blank.
pub(crate) fn equal_lane(lane: &[LaneRecord]) -> Vec<LaneBlock> {
    let count = lane.len();
    if count == 0 {
        return Vec::new();
    }
    let share = 1.0 / count as f32;
    lane.iter()
        .enumerate()
        .map(|(slot, record)| LaneBlock {
            index: record.index,
            turn: record.turn,
            x: slot as f32 * share,
            width: share,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{equal_lane, time_lane, LaneRecord, TimeAxis};

    fn record(index: usize, start: Option<u64>, duration_ms: Option<u64>) -> LaneRecord {
        LaneRecord {
            index,
            turn: 1,
            start,
            duration_ms,
        }
    }

    /// The axis is the union of every lane, not the span of the one being
    /// drawn, so blocks line up ACROSS lanes.
    #[test]
    fn the_axis_spans_every_lane() {
        let input = vec![record(0, Some(5_000), Some(4_000))];
        let model = vec![record(1, Some(1_000), Some(9_000))];
        let tools = vec![record(2, Some(20_000), Some(200))];
        let axis = TimeAxis::across(&[&input, &model, &tools]).expect("a clock");
        assert_eq!(axis.start, 1_000, "the earliest start anywhere");
        assert_eq!(axis.end, 20_200, "the latest end anywhere");
    }

    /// A block starts where its event started and is as wide as the work took;
    /// the 9s and 0.2s turns are 45x apart, and 10s apart on the axis.
    #[test]
    fn duration_places_by_start_and_sizes_by_duration() {
        let lane = vec![
            record(0, Some(1_000), Some(9_000)),
            record(1, Some(11_000), Some(200)),
        ];
        let axis = TimeAxis::across(&[&lane]).expect("a clock");
        let blocks = time_lane(&lane, axis);
        assert_eq!(blocks.len(), 2, "neither event is dropped");
        assert_eq!(blocks[0].x, 0.0, "the first event sits on the axis start");
        assert_eq!(blocks[0].width, 9_000.0 / 10_200.0);
        assert_eq!(blocks[1].width, 200.0 / 10_200.0);
        let ratio = blocks[0].width / blocks[1].width;
        assert!(
            (ratio - 45.0).abs() < 0.01,
            "the slow block is 45x the quick one: {ratio}"
        );
        assert_eq!(
            blocks[1].x,
            10_000.0 / 10_200.0,
            "the quick one starts ten seconds along the axis"
        );
    }

    /// The empty stretch between two events is real space: the next block starts
    /// where its event started, not where the previous one ended.
    #[test]
    fn an_idle_gap_stays_empty() {
        let lane = vec![
            record(0, Some(0), Some(9_000)),
            record(1, Some(19_000), Some(200)),
        ];
        let axis = TimeAxis::across(&[&lane]).expect("a clock");
        let blocks = time_lane(&lane, axis);
        let first_end = blocks[0].x + blocks[0].width;
        assert!(
            first_end < blocks[1].x,
            "the ten-second wait is empty space: {first_end} < {}",
            blocks[1].x
        );
        assert_eq!(
            blocks[1].x,
            19_000.0 / 19_200.0,
            "work resumes at 19s, where the second event really started"
        );
    }

    /// Two lanes that start at the same instant project the same X, which is
    /// what makes the strip read across rather than lane by lane.
    #[test]
    fn a_shared_start_shares_an_x() {
        let input = vec![record(0, Some(5_000), Some(1_000))];
        let tools = vec![record(1, Some(5_000), Some(3_000))];
        let axis = TimeAxis::across(&[&input, &tools]).expect("a clock");
        let left = time_lane(&input, axis);
        let right = time_lane(&tools, axis);
        assert_eq!(left[0].x, right[0].x);
        assert_eq!(left[0].x, 0.0, "both open the axis");
    }

    /// An event with no start is placed beside its nearest timed neighbour and
    /// still gets a block; an event with no duration keeps a zero share that
    /// the renderer raises to its two-pixel floor.
    #[test]
    fn an_untimed_event_is_not_dropped() {
        let lane = vec![
            record(0, Some(1_000), Some(1_000)),
            record(1, None, None),
            record(2, Some(11_000), Some(1_000)),
        ];
        let axis = TimeAxis::across(&[&lane]).expect("the timed records make one");
        let blocks = time_lane(&lane, axis);
        assert_eq!(blocks.len(), 3, "the untimed event still draws");
        assert_eq!(blocks[1].x, blocks[0].x, "it sits by the last known time");
        assert_eq!(blocks[1].width, 0.0, "the renderer applies the 2px floor");
        assert_eq!(blocks[2].x, 10_000.0 / 11_000.0);
    }

    /// An untimed event is placed near the end of the axis, so its own duration
    /// would reach past it. The width is clamped while the left edge stays where
    /// the fallback put it.
    #[test]
    fn an_approximate_block_never_runs_past_the_axis() {
        let lane = vec![
            record(0, Some(0), Some(1_000)),
            record(1, Some(10_000), Some(1)),
            record(2, None, Some(9_000)),
        ];
        let axis = TimeAxis::across(&[&lane]).expect("a clock");
        let blocks = time_lane(&lane, axis);
        let last = blocks[2];
        assert!(
            last.x + last.width <= 1.0 + f32::EPSILON,
            "the fallback block stays inside the lane: {} + {}",
            last.x,
            last.width
        );
        assert!(last.width > 0.0, "it is still drawn");
    }

    /// With no clock anywhere the axis is absent, and the caller can still lay
    /// the lane out as equal shares rather than drawing nothing.
    #[test]
    fn an_untimed_lane_falls_back_to_equal_shares() {
        let lane = vec![record(0, None, None), record(1, None, None)];
        assert!(TimeAxis::across(&[&lane]).is_none());
        let blocks = equal_lane(&lane);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].x, 0.0);
        assert_eq!(blocks[0].width, 0.5);
        assert_eq!(blocks[1].x, 0.5);
    }
}
