//! Leiden community detection over a weighted undirected graph.
//!
//! This is a deterministic implementation of the paper's main pipeline:
//! queue-based local moving, well-connected refinement inside each community,
//! aggregation of the refined partition, and repetition until no aggregation is
//! possible. The best partition by modularity is retained. Node order is stable
//! and ties are broken by the lower community id, so repeated runs agree.

use std::collections::{BTreeMap, VecDeque};

/// Tuning for one Leiden run.
#[derive(Clone, Copy, Debug)]
pub struct LeidenOptions {
    /// Resolution parameter in the modularity gain.
    pub resolution: f64,
    /// Maximum aggregation levels.
    pub max_levels: usize,
    /// Maximum local-moving sweeps per level.
    pub max_sweeps: usize,
}

impl Default for LeidenOptions {
    fn default() -> Self {
        Self {
            resolution: 1.0,
            max_levels: 8,
            max_sweeps: 20,
        }
    }
}

/// One Leiden result.
#[derive(Clone, Debug, PartialEq)]
pub struct LeidenResult {
    /// Community assignment per original node.
    pub assignment: Vec<usize>,
    /// Modularity of the assignment.
    pub modularity: f64,
}

/// Run Leiden on a symmetric weighted graph.
///
/// Weights are indexed by node id; a self-loop is counted once.
#[must_use]
pub fn leiden(weights: &[BTreeMap<usize, f64>], options: LeidenOptions) -> LeidenResult {
    let count = weights.len();
    if count == 0 {
        return LeidenResult {
            assignment: Vec::new(),
            modularity: 0.0,
        };
    }
    let mut current_weights = weights.to_vec();
    let mut mapping: Vec<usize> = (0..count).collect();
    let mut best_assignment: Vec<usize> = (0..count).collect();
    let mut best_modularity = modularity(&best_assignment, weights);

    for _ in 0..options.max_levels {
        let level_nodes = current_weights.len();
        let moved = local_move(&current_weights, options);
        let refined = refine(&moved, &current_weights, options);
        let original_assignment: Vec<usize> = mapping.iter().map(|node| refined[*node]).collect();
        let quality = modularity(&original_assignment, weights);
        if quality > best_modularity + 1e-12 {
            best_modularity = quality;
            best_assignment = original_assignment;
        }

        let (next_weights, next_mapping) = aggregate(&refined, &current_weights);
        if next_weights.len() == level_nodes {
            break;
        }
        mapping = mapping
            .iter()
            .map(|node| next_mapping[refined[*node]])
            .collect();
        current_weights = next_weights;
    }

    LeidenResult {
        assignment: best_assignment,
        modularity: best_modularity,
    }
}

fn local_move(weights: &[BTreeMap<usize, f64>], options: LeidenOptions) -> Vec<usize> {
    let count = weights.len();
    let degree = degrees(weights);
    let m2: f64 = degree.iter().sum();
    if m2 <= 0.0 {
        return (0..count).collect();
    }
    let resolution = options.resolution.max(0.0);

    let mut assignment: Vec<usize> = (0..count).collect();
    let mut totals: Vec<f64> = degree.clone();
    let mut queue: VecDeque<usize> = (0..count).collect();
    let mut queued = vec![true; count];

    for _ in 0..options.max_sweeps {
        let mut moved = false;
        let mut processed = 0usize;
        while let Some(node) = queue.pop_front() {
            processed += 1;
            if processed > count.saturating_mul(100).max(1000) {
                break;
            }
            queued[node] = false;
            let current = assignment[node];
            let k_i = degree[node];
            let mut weights_by_community: BTreeMap<usize, f64> = BTreeMap::new();
            for (neighbor, weight) in &weights[node] {
                if *neighbor == node {
                    continue;
                }
                let community = assignment[*neighbor];
                *weights_by_community.entry(community).or_default() += weight;
            }

            let current_weight = weights_by_community.get(&current).copied().unwrap_or(0.0);
            let current_total = totals[current] - k_i;
            let mut best = current;
            let mut best_gain = current_weight - resolution * current_total * k_i / m2;
            for (community, weight) in &weights_by_community {
                if *community == current {
                    continue;
                }
                let gain = *weight - resolution * totals[*community] * k_i / m2;
                if gain > best_gain + 1e-12
                    || ((gain - best_gain).abs() <= 1e-12 && *community < best)
                {
                    best = *community;
                    best_gain = gain;
                }
            }

            if best != current {
                assignment[node] = best;
                totals[current] -= k_i;
                totals[best] += k_i;
                moved = true;
                for neighbor in weights[node].keys() {
                    if !queued[*neighbor] {
                        queue.push_back(*neighbor);
                        queued[*neighbor] = true;
                    }
                }
            }
        }
        if !moved {
            break;
        }
    }
    assignment
}

fn refine(
    assignment: &[usize],
    weights: &[BTreeMap<usize, f64>],
    options: LeidenOptions,
) -> Vec<usize> {
    let count = weights.len();
    let degree = degrees(weights);
    let m2: f64 = degree.iter().sum();
    if m2 <= 0.0 {
        return (0..count).collect();
    }
    let resolution = options.resolution.max(0.0);

    let mut communities: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (node, community) in assignment.iter().enumerate() {
        communities.entry(*community).or_default().push(node);
    }

    let mut parent: Vec<usize> = (0..count).collect();
    let mut totals: Vec<f64> = degree.clone();
    for members in communities.values() {
        for node in members {
            let mut by_community: BTreeMap<usize, f64> = BTreeMap::new();
            for (neighbor, weight) in &weights[*node] {
                if *neighbor == *node || assignment[*neighbor] != assignment[*node] {
                    continue;
                }
                let root = find(&mut parent, *neighbor);
                *by_community.entry(root).or_default() += weight;
            }
            let own = find(&mut parent, *node);
            let own_weight = by_community.get(&own).copied().unwrap_or(0.0);
            let mut best = own;
            let mut best_gain =
                own_weight - resolution * (totals[own] - degree[*node]) * degree[*node] / m2;
            for (root, weight) in &by_community {
                if *root == own {
                    continue;
                }
                let gain = *weight - resolution * totals[*root] * degree[*node] / m2;
                if gain > best_gain + 1e-12 || ((gain - best_gain).abs() <= 1e-12 && *root < best) {
                    best = *root;
                    best_gain = gain;
                }
            }
            if best != own && best_gain > 0.0 {
                let left = find(&mut parent, own);
                let right = find(&mut parent, best);
                if left != right {
                    let (low, high) = if left < right {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    parent[high] = low;
                    totals[low] += totals[high];
                    totals[high] = 0.0;
                }
            }
        }
    }

    let mut roots: BTreeMap<usize, usize> = BTreeMap::new();
    (0..count)
        .map(|node| {
            let root = find(&mut parent, node);
            let next = roots.len();
            *roots.entry(root).or_insert(next)
        })
        .collect()
}

fn aggregate(
    assignment: &[usize],
    weights: &[BTreeMap<usize, f64>],
) -> (Vec<BTreeMap<usize, f64>>, Vec<usize>) {
    let mut map: BTreeMap<usize, usize> = BTreeMap::new();
    for community in assignment {
        let next = map.len();
        map.entry(*community).or_insert(next);
    }
    let mut super_weights: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); map.len()];
    for (from, neighbors) in weights.iter().enumerate() {
        let from_super = map[&assignment[from]];
        for (to, weight) in neighbors {
            if to < &from {
                continue;
            }
            let to_super = map[&assignment[*to]];
            if from_super == to_super {
                *super_weights[from_super].entry(from_super).or_default() += weight;
            } else {
                *super_weights[from_super].entry(to_super).or_default() += weight;
                *super_weights[to_super].entry(from_super).or_default() += weight;
            }
        }
    }
    (
        super_weights,
        assignment.iter().map(|community| map[community]).collect(),
    )
}

fn degrees(weights: &[BTreeMap<usize, f64>]) -> Vec<f64> {
    weights
        .iter()
        .enumerate()
        .map(|(node, neighbors)| {
            let mut total: f64 = neighbors.values().sum();
            if let Some(weight) = neighbors.get(&node) {
                total += weight;
            }
            total
        })
        .collect()
}

fn modularity(assignment: &[usize], weights: &[BTreeMap<usize, f64>]) -> f64 {
    let m2: f64 = degrees(weights).iter().sum();
    if m2 <= 0.0 {
        return 0.0;
    }
    let m = m2 / 2.0;
    let mut internal: BTreeMap<usize, f64> = BTreeMap::new();
    let mut totals: BTreeMap<usize, f64> = BTreeMap::new();
    for (node, neighbors) in weights.iter().enumerate() {
        let degree: f64 = neighbors.values().sum::<f64>();
        *totals.entry(assignment[node]).or_default() += degree;
        for (neighbor, weight) in neighbors {
            if neighbor < &node {
                continue;
            }
            if assignment[node] == assignment[*neighbor] {
                *internal.entry(assignment[node]).or_default() += weight;
            }
        }
    }
    internal
        .iter()
        .map(|(community, weight)| {
            let degree = totals.get(community).copied().unwrap_or(0.0);
            weight / m - (degree / m2).powi(2)
        })
        .sum()
}

fn find(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(usize, usize)]) -> Vec<BTreeMap<usize, f64>> {
        let count = edges.iter().flat_map(|(a, b)| [*a, *b]).max().unwrap_or(0) + 1;
        let mut weights = vec![BTreeMap::new(); count];
        for (a, b) in edges {
            *weights[*a].entry(*b).or_default() += 1.0;
            *weights[*b].entry(*a).or_default() += 1.0;
        }
        weights
    }

    #[test]
    fn separates_two_dense_components() {
        let weights = graph(&[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)]);
        let result = leiden(&weights, LeidenOptions::default());
        assert_eq!(result.assignment[0], result.assignment[1]);
        assert_eq!(result.assignment[0], result.assignment[2]);
        assert_eq!(result.assignment[3], result.assignment[4]);
        assert_eq!(result.assignment[3], result.assignment[5]);
        assert_ne!(result.assignment[0], result.assignment[3]);
        assert!(result.modularity > 0.0);
    }

    #[test]
    fn deterministic_for_same_input() {
        let weights = graph(&[(0, 1), (1, 2), (2, 3), (3, 0), (0, 2)]);
        let first = leiden(&weights, LeidenOptions::default());
        let second = leiden(&weights, LeidenOptions::default());
        assert_eq!(first, second);
    }
}
