//! Deterministic projections of a candidate graph.
//!
//! Reports, wiki pages, and benchmark numbers are projections over the same
//! `CodeGraph`. They never become a second source of truth and never invent
//! edges that the graph does not contain.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::graph::{CodeGraph, EdgeKind, GodNode, NodeId, NodeKind};

/// One exported text file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExportedDocument {
    /// Relative output path.
    pub path: String,
    /// UTF-8 content.
    pub content: String,
}

/// One connected candidate community.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Community {
    /// Numeric community id, stable for one graph.
    pub id: usize,
    /// Human-readable label: the highest-degree member's name.
    pub label: String,
    /// Member nodes in identity order.
    pub nodes: Vec<NodeId>,
}

/// Token comparison between raw source and the graph snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphBenchmark {
    /// Bytes of the source corpus supplied by the caller.
    pub raw_bytes: usize,
    /// Bytes of the serialized graph snapshot.
    pub graph_bytes: usize,
    /// Estimated raw tokens (4 bytes per token).
    pub raw_tokens: usize,
    /// Estimated graph tokens.
    pub graph_tokens: usize,
    /// Raw tokens divided by graph tokens.
    pub ratio: f64,
}

impl CodeGraph {
    /// Export the graph snapshot as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns a serialization error when the snapshot cannot be encoded.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.to_snapshot())
    }

    /// Render a Markdown report with coverage, god nodes, and communities.
    #[must_use]
    pub fn to_markdown(&self, title: &str) -> String {
        let mut out = format!("# {title}\n\n");
        out.push_str(&format!("Workspace: {}\n\n", self.workspace_id()));
        let coverage = self.coverage();
        out.push_str("## Coverage\n\n");
        out.push_str(&format!(
            "- nodes: {}\n- edges: {}\n- inferred edges: {}\n- ambiguous edges: {}\n- unresolved references: {}\n- unresolved relations: {}\n- complete: {}\n\n",
            self.nodes().count(),
            self.edges().len(),
            coverage.inferred_edges,
            coverage.ambiguous_edges,
            coverage.unresolved_references,
            coverage.unresolved_relations,
            coverage.is_complete(),
        ));

        out.push_str("## God nodes\n\n");
        out.push_str("| node | kind | degree | path |\n|---|---|---|---|\n");
        for GodNode {
            node,
            degree,
            incoming,
            outgoing,
        } in self.god_nodes(10)
        {
            out.push_str(&format!(
                "| {} | {} | {} ({} in / {} out) | {} |\n",
                node.name,
                kind_label(node.kind),
                degree,
                incoming,
                outgoing,
                node.path.display()
            ));
        }
        out.push('\n');

        out.push_str("## Communities\n\n");
        for community in self.communities().iter().take(10) {
            let names = community
                .nodes
                .iter()
                .filter_map(|id| self.node(id))
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "- community {}: {} ({} nodes): {}\n",
                community.id,
                community.label,
                community.nodes.len(),
                names
            ));
        }
        out.push('\n');

        out.push_str("## Edge kinds\n\n");
        let mut counts: BTreeMap<EdgeKind, usize> = BTreeMap::new();
        for edge in self.edges() {
            *counts.entry(edge.kind).or_default() += 1;
        }
        for (kind, count) in counts {
            out.push_str(&format!("- {}: {count}\n", kind.as_str()));
        }
        out
    }

    /// Render a Markdown index plus one page per community.
    #[must_use]
    pub fn to_wiki(&self, title: &str) -> Vec<ExportedDocument> {
        let mut documents = vec![ExportedDocument {
            path: "index.md".to_owned(),
            content: self.to_markdown(title),
        }];
        for community in self.communities().iter().take(50) {
            let mut content = format!("# Community {}: {}\n\n", community.id, community.label);
            for id in &community.nodes {
                if let Some(node) = self.node(id) {
                    content.push_str(&format!(
                        "- {} ({}) at {}\n",
                        node.name,
                        kind_label(node.kind),
                        node.path.display()
                    ));
                }
            }
            documents.push(ExportedDocument {
                path: format!("community-{}.md", community.id),
                content,
            });
        }
        documents
    }

    /// Compare raw source size with the serialized graph size.
    #[must_use]
    pub fn benchmark(&self, raw_bytes: usize) -> GraphBenchmark {
        let graph_bytes = self.to_json().map(|json| json.len()).unwrap_or_default();
        let raw_tokens = tokens(raw_bytes);
        let graph_tokens = tokens(graph_bytes);
        GraphBenchmark {
            raw_bytes,
            graph_bytes,
            raw_tokens,
            graph_tokens,
            ratio: raw_tokens as f64 / graph_tokens.max(1) as f64,
        }
    }

    /// Partition nodes into modularity communities with labels.
    ///
    /// This is a deterministic Leiden-style approximation: local moving on an
    /// undirected weighted graph, followed by a connected-component refinement
    /// inside each community. It reports modularity, but it is not the full
    /// Leiden aggregation/bisection pipeline.
    #[must_use]
    pub fn communities(&self) -> Vec<Community> {
        let (assignment, _) = self.assign_communities();
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (position, community) in assignment.iter().enumerate() {
            groups.entry(*community).or_default().push(position);
        }
        let nodes: Vec<&crate::graph::GraphNode> = self.nodes().collect();
        let mut degrees: BTreeMap<&NodeId, usize> = BTreeMap::new();
        for edge in self.edges() {
            *degrees.entry(&edge.from).or_default() += 1;
            *degrees.entry(&edge.to).or_default() += 1;
        }
        let mut communities: Vec<Community> = groups
            .into_values()
            .map(|members| {
                let label_index = members
                    .iter()
                    .copied()
                    .max_by(|left, right| {
                        degrees
                            .get(&nodes[*left].id)
                            .cmp(&degrees.get(&nodes[*right].id))
                            .then_with(|| nodes[*left].name.cmp(&nodes[*right].name))
                    })
                    .unwrap_or(0);
                Community {
                    id: 0,
                    label: nodes[label_index].name.clone(),
                    nodes: members
                        .iter()
                        .map(|index| nodes[*index].id.clone())
                        .collect(),
                }
            })
            .collect();
        communities.sort_by(|left, right| {
            right
                .nodes
                .len()
                .cmp(&left.nodes.len())
                .then_with(|| left.label.cmp(&right.label))
        });
        for (id, community) in communities.iter_mut().enumerate() {
            community.id = id;
        }
        communities
    }

    /// Modularity of the current community assignment.
    #[must_use]
    pub fn modularity(&self) -> f64 {
        self.assign_communities().1
    }

    fn assign_communities(&self) -> (Vec<usize>, f64) {
        let nodes: Vec<&crate::graph::GraphNode> = self.nodes().collect();
        let count = nodes.len();
        if count == 0 {
            return (Vec::new(), 0.0);
        }
        let mut index: BTreeMap<&NodeId, usize> = BTreeMap::new();
        for (position, node) in nodes.iter().enumerate() {
            index.insert(&node.id, position);
        }
        let mut weights: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); count];
        for edge in self.edges() {
            let (Some(&from), Some(&to)) = (index.get(&edge.from), index.get(&edge.to)) else {
                continue;
            };
            if from == to {
                continue;
            }
            *weights[from].entry(to).or_default() += 1.0;
            *weights[to].entry(from).or_default() += 1.0;
        }

        let result = crate::leiden::leiden(&weights, crate::leiden::LeidenOptions::default());
        (result.assignment, result.modularity)
    }
}

fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::File => "file",
        NodeKind::Symbol(symbol) => symbol.as_str(),
    }
}

fn tokens(bytes: usize) -> usize {
    bytes.saturating_add(3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceId;
    use crate::graph::{
        EdgeKind, EvidenceClass, GraphCoverage, GraphEdge, GraphNode, GraphSnapshot, NodeId,
        NodeKind,
    };
    use crate::resolution::SymbolKind;
    use crate::syntax::SourceRange;
    use std::path::PathBuf;

    fn graph() -> CodeGraph {
        let workspace = WorkspaceId::new();
        let path = PathBuf::from("src/lib.rs");
        let file = NodeId::file(&path);
        let alpha = NodeId::symbol(&crate::SymbolId {
            workspace_id: workspace,
            path: path.clone(),
            start_byte: 0,
            end_byte: 10,
            kind: SymbolKind::Function,
        });
        let beta = NodeId::symbol(&crate::SymbolId {
            workspace_id: workspace,
            path: path.clone(),
            start_byte: 20,
            end_byte: 30,
            kind: SymbolKind::Function,
        });
        let snapshot = GraphSnapshot {
            workspace_id: workspace.to_string(),
            nodes: vec![
                GraphNode {
                    id: file.clone(),
                    kind: NodeKind::File,
                    name: "src/lib.rs".to_owned(),
                    path: path.clone(),
                    range: None,
                },
                GraphNode {
                    id: alpha.clone(),
                    kind: NodeKind::Symbol(SymbolKind::Function),
                    name: "alpha".to_owned(),
                    path: path.clone(),
                    range: Some(SourceRange {
                        start_byte: 0,
                        end_byte: 10,
                    }),
                },
                GraphNode {
                    id: beta.clone(),
                    kind: NodeKind::Symbol(SymbolKind::Function),
                    name: "beta".to_owned(),
                    path,
                    range: Some(SourceRange {
                        start_byte: 20,
                        end_byte: 30,
                    }),
                },
            ],
            edges: vec![
                GraphEdge {
                    from: file.clone(),
                    to: alpha.clone(),
                    kind: EdgeKind::Contains,
                    class: EvidenceClass::Extracted,
                    references: Vec::new(),
                },
                GraphEdge {
                    from: file,
                    to: beta.clone(),
                    kind: EdgeKind::Contains,
                    class: EvidenceClass::Extracted,
                    references: Vec::new(),
                },
                GraphEdge {
                    from: alpha,
                    to: beta,
                    kind: EdgeKind::Calls,
                    class: EvidenceClass::Inferred,
                    references: Vec::new(),
                },
            ],
            coverage: GraphCoverage {
                symbols: 2,
                files: 1,
                edges: 3,
                inferred_edges: 1,
                ..GraphCoverage::default()
            },
            source_digest: String::new(),
            metadata: std::collections::BTreeMap::new(),
        };
        CodeGraph::from_snapshot(snapshot).expect("graph")
    }

    #[test]
    fn markdown_wiki_and_benchmark_are_deterministic() {
        let graph = graph();
        let markdown = graph.to_markdown("codel");
        assert!(markdown.contains("God nodes"));
        assert!(markdown.contains("Communities"));
        assert!(markdown.contains("calls"));
        let wiki = graph.to_wiki("codel");
        assert_eq!(wiki[0].path, "index.md");
        assert!(
            wiki.iter()
                .any(|document| document.path.starts_with("community-"))
        );
        let communities = graph.communities();
        assert!(!communities.is_empty());
        assert!(
            communities
                .iter()
                .all(|community| !community.label.is_empty())
        );
        assert!(graph.modularity() >= 0.0);
        let benchmark = graph.benchmark(4_000);
        assert_eq!(benchmark.raw_tokens, 1_000);
        assert!(benchmark.ratio > 0.0);
    }
    #[test]
    fn additional_formats_render() {
        let graph = graph();
        for format in ["html", "svg", "graphml", "neo4j", "obsidian", "callflow"] {
            let documents = graph.export(format, "codel").expect("export");
            assert!(!documents.is_empty(), "empty {format}");
        }
        assert!(graph.to_svg().contains("<svg"));
        assert!(graph.to_graphml().contains("<graphml"));
        assert!(graph.to_neo4j().contains("CREATE"));
        assert!(graph.to_callflow().contains("mermaid"));
        assert!(
            graph
                .to_obsidian("codel")
                .iter()
                .all(|doc| doc.path.starts_with("obsidian/"))
        );
    }
}
