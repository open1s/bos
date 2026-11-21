//! Graph diagnostics: cycles and dead-code candidates.

use std::collections::{BTreeMap, BTreeSet};

use codel::graph::{CodeGraph, EdgeKind, NodeId};
use serde_json::{Value, json};

/// Find strongly connected components larger than one node (and self-loops).
#[must_use]
pub fn cycles(graph: &CodeGraph, kinds: &[EdgeKind], max: usize) -> Value {
    let adjacency = adjacency(graph, kinds);
    let mut state = Tarjan::default();
    for node in graph.nodes() {
        if !state.indices.contains_key(&node.id) {
            strong_connect(&node.id, &adjacency, &mut state);
        }
    }
    let mut cycles = state
        .components
        .into_iter()
        .filter(|component| {
            component.len() > 1
                || (component.len() == 1
                    && adjacency
                        .get(&component[0])
                        .is_some_and(|targets| targets.contains(&component[0])))
        })
        .map(|component| {
            let nodes = component
                .iter()
                .filter_map(|id| graph.node(id))
                .map(|node| {
                    json!({
                        "id": node.id.as_str(),
                        "name": node.name,
                        "path": node.path,
                        "kind": node.kind,
                    })
                })
                .collect::<Vec<_>>();
            json!({"size": component.len(), "nodes": nodes})
        })
        .collect::<Vec<_>>();
    cycles.sort_by_key(|cycle| std::cmp::Reverse(cycle["size"].as_u64().unwrap_or(0)));
    cycles.truncate(max);
    json!({"count": cycles.len(), "cycles": cycles})
}

/// Reverse-reachability from symbols declared in changed files.
#[must_use]
pub fn impact(graph: &CodeGraph, changed: &[String], max_depth: usize, max_nodes: usize) -> Value {
    let changed: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
    let mut seeds = Vec::new();
    for node in graph.nodes() {
        if changed.contains(node.path.to_string_lossy().as_ref()) {
            seeds.push(node.id.clone());
        }
    }

    let mut reverse: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
    for edge in graph.edges() {
        if edge.kind != EdgeKind::Contains {
            reverse.entry(&edge.to).or_default().push(&edge.from);
        }
    }

    let mut seen: BTreeSet<NodeId> = seeds.iter().cloned().collect();
    let mut queue: Vec<(NodeId, usize)> = seeds.iter().cloned().map(|id| (id, 0)).collect();
    let mut affected = Vec::new();
    let mut index = 0;
    while index < queue.len() && affected.len() < max_nodes {
        let (id, depth) = queue[index].clone();
        index += 1;
        if depth > 0
            && let Some(node) = graph.node(&id)
        {
            affected.push(json!({
                "id": node.id.as_str(),
                "name": node.name,
                "path": node.path,
                "kind": node.kind,
                "depth": depth,
            }));
        }
        if depth >= max_depth {
            continue;
        }
        if let Some(parents) = reverse.get(&id) {
            for parent in parents {
                if seen.insert((*parent).clone()) {
                    queue.push(((*parent).clone(), depth + 1));
                }
            }
        }
    }

    let direct = seeds
        .iter()
        .filter_map(|id| graph.node(id))
        .map(|node| {
            json!({
                "id": node.id.as_str(),
                "name": node.name,
                "path": node.path,
                "kind": node.kind,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "changedFiles": changed,
        "changedNodes": direct,
        "affected": affected,
        "truncated": affected.len() >= max_nodes,
    })
}

/// Symbol nodes with no incoming non-containment edge.
#[must_use]
pub fn dead_code(graph: &CodeGraph, max: usize) -> Value {
    let mut incoming: BTreeMap<&NodeId, usize> = BTreeMap::new();
    for edge in graph.edges() {
        if edge.kind != EdgeKind::Contains {
            *incoming.entry(&edge.to).or_default() += 1;
        }
    }
    let mut candidates = graph
        .nodes()
        .filter(|node| node.kind.is_symbol())
        .filter(|node| incoming.get(&node.id).copied().unwrap_or(0) == 0)
        .filter(|node| !is_entrypoint(&node.name))
        .map(|node| {
            json!({
                "id": node.id.as_str(),
                "name": node.name,
                "path": node.path,
                "kind": node.kind,
                "reason": "no incoming non-containment edges",
                "confidence": "candidate",
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left["path"]
            .as_str()
            .cmp(&right["path"].as_str())
            .then(left["name"].as_str().cmp(&right["name"].as_str()))
    });
    candidates.truncate(max);
    json!({"count": candidates.len(), "candidates": candidates})
}

fn is_entrypoint(name: &str) -> bool {
    matches!(name, "main" | "new" | "default" | "drop")
        || name.starts_with("test_")
        || name.ends_with("_test")
        || name.starts_with("__")
}

fn adjacency(graph: &CodeGraph, kinds: &[EdgeKind]) -> BTreeMap<NodeId, Vec<NodeId>> {
    let mut adjacency: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for edge in graph.edges() {
        if kinds.is_empty() || kinds.contains(&edge.kind) {
            adjacency
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }
    }
    adjacency
}

#[derive(Default)]
struct Tarjan {
    next: usize,
    indices: BTreeMap<NodeId, usize>,
    low: BTreeMap<NodeId, usize>,
    stack: Vec<NodeId>,
    on_stack: BTreeSet<NodeId>,
    components: Vec<Vec<NodeId>>,
}

fn strong_connect(node: &NodeId, adjacency: &BTreeMap<NodeId, Vec<NodeId>>, state: &mut Tarjan) {
    let index = state.next;
    state.next += 1;
    state.indices.insert(node.clone(), index);
    state.low.insert(node.clone(), index);
    state.stack.push(node.clone());
    state.on_stack.insert(node.clone());

    if let Some(targets) = adjacency.get(node) {
        for target in targets {
            if !state.indices.contains_key(target) {
                strong_connect(target, adjacency, state);
                let low = state.low[target];
                let current = state.low[node];
                state.low.insert(node.clone(), current.min(low));
            } else if state.on_stack.contains(target) {
                let target_index = state.indices[target];
                let current = state.low[node];
                state.low.insert(node.clone(), current.min(target_index));
            }
        }
    }

    if state.low[node] == state.indices[node] {
        let mut component = Vec::new();
        while let Some(member) = state.stack.pop() {
            state.on_stack.remove(&member);
            component.push(member.clone());
            if &member == node {
                break;
            }
        }
        state.components.push(component);
    }
}
