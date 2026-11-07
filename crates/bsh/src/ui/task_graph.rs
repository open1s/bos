//! The Tasks graph: a main-agent node over its tasks and subagent children.
//!
//! Pure layout so the pane only paints. Two records feed it: the durable task
//! store (what the model created) and the live subagent runs (what is executing
//! right now). A subagent-tier task is represented by its run, not twice.

use crate::extension::tasks::{Task, TaskStatus, Tier};
use crate::subagent::{SubagentRun, SubagentStatus};

/// What a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// The conversation itself.
    Main,
    /// A delegated child run.
    Subagent,
    /// A durable task from the store.
    Task,
}

/// The display state of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GraphStatus {
    /// Live.
    Running,
    /// Waiting for a resource.
    Queued,
    /// Between firings.
    Scheduled,
    /// Waiting for an approval.
    Blocked,
    /// Finished successfully.
    Done,
    /// Finished with an error.
    Failed,
    /// Stopped.
    Cancelled,
}

impl GraphStatus {
    /// The label key.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Running => "right.tasks.running",
            Self::Queued => "right.tasks.queued",
            Self::Scheduled => "right.tasks.scheduled",
            Self::Blocked => "right.tasks.blocked",
            Self::Done => "right.tasks.done",
            Self::Failed => "right.tasks.failed",
            Self::Cancelled => "right.tasks.cancelled",
        }
    }
}

/// One placed node.
#[derive(Clone, Debug)]
pub(crate) struct GraphNode {
    /// The run id, the task id, or the session id for the main node.
    pub(crate) id: String,
    /// What it is.
    pub(crate) kind: NodeKind,
    /// The label the card shows.
    pub(crate) title: String,
    /// A second label: the tier for a task, or "continuable" for a child.
    pub(crate) subtitle: Option<&'static str>,
    /// The run state.
    pub(crate) status: GraphStatus,
    /// Left edge, in unscaled pixels.
    pub(crate) x: f32,
    /// Top edge, in unscaled pixels.
    pub(crate) y: f32,
    /// Depth in the tree.
    pub(crate) depth: usize,
}

/// One connector: parent index to child index.
pub(crate) type Edge = (usize, usize);

/// The whole graph.
#[derive(Clone, Debug, Default)]
pub(crate) struct Graph {
    /// Nodes, parents before children.
    pub(crate) nodes: Vec<GraphNode>,
    /// Connectors between them.
    pub(crate) edges: Vec<Edge>,
    /// Unscaled content size.
    pub(crate) width: f32,
    /// Unscaled content height.
    pub(crate) height: f32,
}

/// Card geometry, in unscaled pixels.
pub(crate) const CARD_W: f32 = 260.0;
/// Card height.
pub(crate) const CARD_H: f32 = 96.0;
/// Horizontal gap between siblings.
pub(crate) const GAP_X: f32 = 40.0;
/// Vertical gap between levels.
pub(crate) const GAP_Y: f32 = 84.0;
/// Margin around the content inside the canvas.
pub(crate) const MARGIN: f32 = 32.0;

/// The zoom that fits a graph into the pane, never magnifying it.
///
/// A graph smaller than the pane keeps 1.0 so the cards stay at their measured
/// size; a wider one scales down, but never below the readable floor.
pub(crate) fn fit_zoom(graph_w: f32, graph_h: f32, available_w: f32, available_h: f32) -> f32 {
    if graph_w <= 0.0 || graph_h <= 0.0 || available_w <= 0.0 || available_h <= 0.0 {
        return 1.0;
    }
    let scale = (available_w / graph_w).min(available_h / graph_h);
    scale.clamp(0.4, 1.0)
}

/// Build the graph for one session.
pub(crate) fn layout(
    session: &str,
    main_title: &str,
    main_status: GraphStatus,
    tasks: &[Task],
    runs: &[SubagentRun],
    max_width: Option<f32>,
) -> Graph {
    let mut graph = Graph::default();
    graph.nodes.push(GraphNode {
        id: session.to_string(),
        kind: NodeKind::Main,
        title: main_title.to_string(),
        subtitle: None,
        status: main_status,
        x: 0.0,
        y: 0.0,
        depth: 0,
    });

    let mut children: Vec<&SubagentRun> =
        runs.iter().filter(|run| run.session == session).collect();
    children.sort_by(|left, right| {
        (left.depth, left.started_at, &left.id).cmp(&(right.depth, right.started_at, &right.id))
    });
    let run_ids: Vec<&str> = children.iter().map(|run| run.id.as_str()).collect();

    // Durable tasks first. A subagent task whose run is present is drawn by the
    // run itself; everything else gets its own card.
    let mut task_nodes: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.origin.session == session)
        .collect();
    task_nodes
        .sort_by(|left, right| (left.created_ms, &left.id).cmp(&(right.created_ms, &right.id)));
    for task in &task_nodes {
        if task.tier == Tier::Subagent {
            let covered = task
                .attempts
                .last()
                .and_then(|attempt| attempt.run.as_deref())
                .map(|run| run_ids.contains(&run))
                .unwrap_or(false);
            if covered {
                continue;
            }
        }
        let index = graph.nodes.len();
        graph.nodes.push(GraphNode {
            id: task.id.clone(),
            kind: NodeKind::Task,
            title: task.title.clone(),
            subtitle: Some(tier_key(task.tier)),
            status: task_status(task.status),
            x: 0.0,
            y: 0.0,
            depth: 1,
        });
        graph.edges.push((0, index));
    }

    // Then the live runs. A depth-one run hangs under the main node; a deeper
    // run hangs under the nearest ancestor at the previous depth.
    for run in children {
        let parent = if run.depth <= 1 {
            0
        } else {
            graph
                .nodes
                .iter()
                .enumerate()
                .rev()
                .find(|(_, node)| node.kind == NodeKind::Subagent && node.depth == run.depth - 1)
                .map(|(index, _)| index)
                .unwrap_or(0)
        };
        let index = graph.nodes.len();
        graph.nodes.push(GraphNode {
            id: run.id.clone(),
            kind: NodeKind::Subagent,
            title: run.title.clone(),
            subtitle: Some("right.tasks.continuable"),
            status: run_status(run.status),
            x: 0.0,
            y: 0.0,
            depth: run.depth,
        });
        graph.edges.push((parent, index));
    }
    place(&mut graph);
    if let Some(max_width) = max_width {
        wrap(&mut graph, (max_width - MARGIN * 2.0).max(CARD_W));
    }
    graph
}

/// Reflow depth-one nodes into staggered rows when the tree is wider than the
/// pane. Odd rows sit half a gap right and two-fifths of a card down, so the
/// rows brick instead of colliding and the connectors keep room to curve.
fn wrap(graph: &mut Graph, inner: f32) {
    let per_row = (((inner + GAP_X) / (CARD_W + GAP_X)).floor() as usize).max(1);
    let depth_one: Vec<usize> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.depth == 1)
        .map(|(index, _)| index)
        .collect();
    if depth_one.len() <= per_row {
        return;
    }
    let mut first_row_right = 0.0f32;
    for (position, index) in depth_one.iter().enumerate() {
        let row = position / per_row;
        let column = position % per_row;
        let odd = row % 2 == 1;
        let x = column as f32 * (CARD_W + GAP_X) + if odd { GAP_X / 2.0 } else { 0.0 };
        let y = (row + 1) as f32 * (CARD_H + GAP_Y) + if odd { CARD_H * 0.4 } else { 0.0 };
        let node = &mut graph.nodes[*index];
        node.x = x;
        node.y = y;
        if row == 0 {
            first_row_right = first_row_right.max(x + CARD_W);
        }
    }
    // A nested run hangs under the parent it was drawn with.
    for (parent, child) in graph.edges.clone() {
        if graph.nodes[child].depth > 1 {
            let parent_x = graph.nodes[parent].x;
            let parent_y = graph.nodes[parent].y;
            graph.nodes[child].x = parent_x;
            graph.nodes[child].y = parent_y + CARD_H + GAP_Y;
        }
    }
    graph.nodes[0].x = ((first_row_right - CARD_W) / 2.0).max(0.0);
    graph.nodes[0].y = 0.0;
    let mut width = 0.0f32;
    let mut height = 0.0f32;
    for node in &graph.nodes {
        width = width.max(node.x + CARD_W);
        height = height.max(node.y + CARD_H);
    }
    graph.width = width;
    graph.height = height;
}

/// The status for a task record.
fn task_status(status: TaskStatus) -> GraphStatus {
    match status {
        TaskStatus::Running => GraphStatus::Running,
        TaskStatus::Queued | TaskStatus::Waiting => GraphStatus::Queued,
        TaskStatus::Scheduled => GraphStatus::Scheduled,
        TaskStatus::Blocked => GraphStatus::Blocked,
        TaskStatus::Succeeded => GraphStatus::Done,
        TaskStatus::Failed | TaskStatus::Interrupted => GraphStatus::Failed,
        TaskStatus::Cancelled => GraphStatus::Cancelled,
        TaskStatus::Unknown => GraphStatus::Queued,
    }
}

/// The status for a run.
fn run_status(status: SubagentStatus) -> GraphStatus {
    match status {
        SubagentStatus::Running => GraphStatus::Running,
        SubagentStatus::Done => GraphStatus::Done,
        SubagentStatus::Failed => GraphStatus::Failed,
    }
}

/// The tier's label key.
fn tier_key(tier: Tier) -> &'static str {
    match tier {
        Tier::Turn => "right.tasks.tierTurn",
        Tier::Subagent => "right.tasks.tierSubagent",
        Tier::Job => "right.tasks.tierJob",
        Tier::Delegation => "right.tasks.tierDelegation",
        Tier::PeerTask => "right.tasks.tierPeerTask",
    }
}

/// Lay out the tree: leaves take columns, parents center over their children.
fn place(graph: &mut Graph) {
    let mut next_leaf_col = 0usize;
    let mut columns: Vec<f32> = vec![0.0; graph.nodes.len()];
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); graph.nodes.len()];
    for (parent, child) in &graph.edges {
        if let Some(list) = children_of.get_mut(*parent) {
            list.push(*child);
        }
    }
    fn walk(
        node: usize,
        children_of: &[Vec<usize>],
        columns: &mut [f32],
        next_leaf_col: &mut usize,
    ) -> f32 {
        let children = &children_of[node];
        let x = if children.is_empty() {
            let x = *next_leaf_col as f32 * (CARD_W + GAP_X);
            *next_leaf_col = (*next_leaf_col).saturating_add(1);
            x
        } else {
            let mut spans = Vec::with_capacity(children.len());
            for child in children {
                spans.push(walk(*child, children_of, columns, next_leaf_col));
            }
            (spans[0] + spans[spans.len() - 1]) / 2.0
        };
        columns[node] = x;
        x
    }
    walk(0, &children_of, &mut columns, &mut next_leaf_col);
    let max_depth = graph.nodes.iter().map(|node| node.depth).max().unwrap_or(0);
    for (index, node) in graph.nodes.iter_mut().enumerate() {
        node.x = columns[index];
        node.y = node.depth as f32 * (CARD_H + GAP_Y);
    }
    graph.width = (next_leaf_col as f32) * (CARD_W + GAP_X) - GAP_X;
    graph.height = (max_depth + 1) as f32 * CARD_H + max_depth as f32 * GAP_Y;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::tasks::{Attempt, Origin, Trigger};
    use crate::subagent::SubagentRun;

    fn run(id: &str, session: &str, depth: usize, title: &str) -> SubagentRun {
        SubagentRun {
            id: id.to_string(),
            session: session.to_string(),
            parent: session.to_string(),
            depth,
            title: title.to_string(),
            prompt: String::new(),
            status: SubagentStatus::Running,
            started_at: 1,
            finished_at: None,
            estimated_tokens: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            result: None,
            error: None,
            revision: 0,
            transcript: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn task(id: &str, session: &str, tier: Tier, run: Option<&str>) -> Task {
        let mut task = Task::open(
            id,
            "Review the diff",
            "intent",
            Origin {
                session: session.to_string(),
                turn: 0,
            },
            Trigger::Now,
            tier,
            1,
        )
        .expect("a task");
        if let Some(run) = run {
            let mut attempt = Attempt::queued("att-1", tier);
            attempt.run = Some(run.to_string());
            task.attempts.push(attempt);
        }
        task
    }

    /// Two children sit side by side under a centered main node.
    #[test]
    fn a_parent_centers_over_two_children() {
        let graph = layout(
            "ses-1",
            "Show u what tools you",
            GraphStatus::Running,
            &[],
            &[
                run("sub-a", "ses-1", 1, "Sleep 10s (A)"),
                run("sub-b", "ses-1", 1, "Sleep 10s (B)"),
            ],
            None,
        );
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.nodes[0].kind, NodeKind::Main);
        let a = &graph.nodes[1];
        let b = &graph.nodes[2];
        assert!(a.x < b.x, "children are ordered");
        assert_eq!(a.y, CARD_H + GAP_Y);
        let expected = (a.x + b.x) / 2.0;
        assert!((graph.nodes[0].x - expected).abs() < 0.5);
    }

    /// A wide graph scales down to the pane; a small one keeps its size.
    #[test]
    fn fit_zoom_never_magnifies_and_scales_down() {
        assert_eq!(fit_zoom(0.0, 0.0, 500.0, 500.0), 1.0);
        assert_eq!(fit_zoom(300.0, 200.0, 600.0, 600.0), 1.0);
        let fitted = fit_zoom(1000.0, 300.0, 500.0, 600.0);
        assert!((fitted - 0.5).abs() < 0.001, "the width constrains it");
        let tall = fit_zoom(400.0, 800.0, 1000.0, 400.0);
        assert!((tall - 0.5).abs() < 0.001, "the height constrains it");
        assert_eq!(fit_zoom(10_000.0, 10_000.0, 100.0, 100.0), 0.4);
    }

    /// A tree wider than the pane wraps into staggered rows instead of
    /// colliding: odd rows sit half a column right and lower.
    #[test]
    fn a_wide_tree_wraps_into_staggered_rows() {
        let runs: Vec<SubagentRun> = (0..6)
            .map(|index| run(&format!("sub-{index}"), "ses-1", 1, "child"))
            .collect();
        let graph = layout(
            "ses-1",
            "main",
            GraphStatus::Running,
            &[],
            &runs,
            Some(700.0),
        );
        assert_eq!(graph.nodes.len(), 7);
        let row0: Vec<&GraphNode> = graph.nodes.iter().skip(1).take(2).collect();
        let row1: Vec<&GraphNode> = graph.nodes.iter().skip(3).take(2).collect();
        assert_eq!(row0[0].y, row0[1].y, "a row shares its baseline");
        assert!(row1[0].y > row0[0].y, "the next row is lower");
        assert!(
            (row1[0].x - GAP_X / 2.0).abs() < 0.5,
            "the odd row is offset right by half a gap"
        );
        assert!(graph.width <= 701.0, "the wrapped tree fits");
    }
    /// A task record is a node of its own, with its tier as the subtitle.
    #[test]
    fn a_store_task_is_a_node() {
        let graph = layout(
            "ses-1",
            "main",
            GraphStatus::Running,
            &[task("task-1", "ses-1", Tier::Job, None)],
            &[],
            None,
        );
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].kind, NodeKind::Task);
        assert_eq!(graph.nodes[1].id, "task-1");
        assert_eq!(graph.nodes[1].subtitle, Some("right.tasks.tierJob"));
        assert_eq!(graph.edges, vec![(0, 1)]);
    }

    /// A subagent task whose run is live is drawn by the run, not twice.
    #[test]
    fn a_subagent_task_is_drawn_by_its_run() {
        let graph = layout(
            "ses-1",
            "main",
            GraphStatus::Running,
            &[task("task-1", "ses-1", Tier::Subagent, Some("sub-a"))],
            &[run("sub-a", "ses-1", 1, "Sleep 10s (A)")],
            None,
        );
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].id, "sub-a");
        assert_eq!(graph.nodes[1].kind, NodeKind::Subagent);
    }

    /// A deeper run hangs under its depth-one parent, not under the root.
    #[test]
    fn a_nested_run_hangs_under_its_parent() {
        let graph = layout(
            "ses-1",
            "main",
            GraphStatus::Running,
            &[],
            &[
                run("sub-a", "ses-1", 1, "A"),
                run("sub-a1", "ses-1", 2, "A.1"),
            ],
            None,
        );
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.edges, vec![(0, 1), (1, 2)]);
    }

    /// Runs from another session are not part of this graph.
    #[test]
    fn another_sessions_runs_are_ignored() {
        let graph = layout(
            "ses-1",
            "main",
            GraphStatus::Running,
            &[task("task-x", "ses-2", Tier::Job, None)],
            &[run("sub-x", "ses-2", 1, "elsewhere")],
            None,
        );
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.edges.is_empty());
    }
}
