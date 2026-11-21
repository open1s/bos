//! Additional text projections for a candidate graph.

use std::collections::BTreeMap;

use crate::export::ExportedDocument;
use crate::graph::{CodeGraph, EdgeKind, GraphNode, NodeKind};

impl CodeGraph {
    /// Render GraphML for Gephi or yEd.
    #[must_use]
    pub fn to_graphml(&self) -> String {
        let mut out = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<graphml xmlns="http://graphml.graphdrawing.org/xmlns">
<graph id="codel" edgedefault="directed">"#,
        );
        out.push('\n');
        for node in self.nodes() {
            out.push_str(&format!(
                r#"<node id="{}"><data key="name">{}</data><data key="kind">{}</data></node>"#,
                escape_xml(node.id.as_str()),
                escape_xml(&node.name),
                kind_label(node.kind)
            ));
            out.push('\n');
        }
        for (index, edge) in self.edges().iter().enumerate() {
            out.push_str(&format!(
                r#"<edge id="e{index}" source="{}" target="{}"><data key="kind">{}</data><data key="class">{}</data></edge>"#,
                escape_xml(edge.from.as_str()),
                escape_xml(edge.to.as_str()),
                edge.kind.as_str(),
                edge.class.as_str()
            ));
            out.push('\n');
        }
        out.push_str("</graph>\n</graphml>\n");
        out
    }

    /// Render Cypher CREATE/MATCH statements for Neo4j.
    #[must_use]
    pub fn to_neo4j(&self) -> String {
        let mut out = String::new();
        for node in self.nodes() {
            out.push_str(&format!(
                "CREATE (n:{} {{id: \"{}\", name: \"{}\", path: \"{}\"}});\n",
                neo4j_label(node.kind),
                escape_cypher(node.id.as_str()),
                escape_cypher(&node.name),
                escape_cypher(&node.path.display().to_string())
            ));
        }
        for edge in self.edges() {
            out.push_str(&format!(
                "MATCH (a {{id: \"{}\"}}), (b {{id: \"{}\"}}) CREATE (a)-[:{} {{class: \"{}\"}}]->(b);\n",
                escape_cypher(edge.from.as_str()),
                escape_cypher(edge.to.as_str()),
                neo4j_label(edge.kind),
                edge.class.as_str()
            ));
        }
        out
    }

    /// Render a Mermaid call-flow page.
    #[must_use]
    pub fn to_callflow(&self) -> String {
        let fence = char::from(96).to_string().repeat(3);
        let mut out = format!("{fence}mermaid\ngraph TD\n");
        for node in self.nodes() {
            out.push_str(&format!(
                "  n{}[\"{}\"]\n",
                sanitize_id(node.id.as_str()),
                escape_mermaid(&node.name)
            ));
        }
        for edge in self.edges() {
            if edge.kind == EdgeKind::Contains {
                continue;
            }
            out.push_str(&format!(
                "  n{} -->|{}| n{}\n",
                sanitize_id(edge.from.as_str()),
                edge.kind.as_str(),
                sanitize_id(edge.to.as_str())
            ));
        }
        out.push_str(&fence);
        out.push('\n');
        out
    }

    /// Render an Obsidian-style vault with frontmatter.
    #[must_use]
    pub fn to_obsidian(&self, title: &str) -> Vec<ExportedDocument> {
        self.to_wiki(title)
            .into_iter()
            .map(|document| ExportedDocument {
                content: format!(
                    "---\ntitle: {}\n---\n\n{}",
                    escape_yaml(title),
                    document.content
                ),
                path: format!("obsidian/{}", document.path),
            })
            .collect()
    }

    /// Render a minimal static HTML report.
    #[must_use]
    pub fn to_html(&self, title: &str) -> String {
        let mut out = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body>\n",
            escape_xml(title)
        );
        out.push_str(&format!("<h1>{}</h1>\n", escape_xml(title)));
        out.push_str("<h2>God nodes</h2><ul>\n");
        for node in self.god_nodes(20) {
            out.push_str(&format!(
                "<li>{} ({}) - degree {}</li>\n",
                escape_xml(&node.node.name),
                kind_label(node.node.kind),
                node.degree
            ));
        }
        out.push_str("</ul>\n<h2>Edges</h2><ul>\n");
        for edge in self.edges() {
            out.push_str(&format!(
                "<li>{}</li>\n",
                escape_xml(&format!(
                    "{} -[{}:{}]-> {}",
                    edge.from,
                    edge.kind.as_str(),
                    edge.class.as_str(),
                    edge.to
                ))
            ));
        }
        out.push_str("</ul>\n</body></html>\n");
        out
    }

    /// Render a deterministic circular node-link SVG.
    #[must_use]
    pub fn to_svg(&self) -> String {
        let nodes: Vec<&GraphNode> = self.nodes().collect();
        let count = nodes.len().max(1);
        let radius = 40.0 + count as f64 * 12.0;
        let center = radius + 40.0;
        let mut positions = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            let angle = std::f64::consts::TAU * index as f64 / count as f64;
            positions.insert(
                node.id.clone(),
                (center + radius * angle.cos(), center + radius * angle.sin()),
            );
        }
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">\n",
            center * 2.0,
            center * 2.0,
            center * 2.0,
            center * 2.0
        );
        out.push_str("<g stroke=\"#999\" stroke-width=\"1\">\n");
        for edge in self.edges() {
            if let (Some((x1, y1)), Some((x2, y2))) =
                (positions.get(&edge.from), positions.get(&edge.to))
            {
                out.push_str(&format!(
                    "<line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" data-kind=\"{}\" />\n",
                    edge.kind.as_str()
                ));
            }
        }
        out.push_str("</g>\n<g font-family=\"monospace\" font-size=\"10\" fill=\"#111\">\n");
        for node in &nodes {
            if let Some((x, y)) = positions.get(&node.id) {
                out.push_str(&format!(
                    "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"5\" fill=\"#4176e6\" />\n<text x=\"{x:.1}\" y=\"{y:.1}\">{}</text>\n",
                    escape_xml(&node.name)
                ));
            }
        }
        out.push_str("</g>\n</svg>\n");
        out
    }

    /// Select one text export format. JSON is handled by the caller.
    #[must_use]
    pub fn export(&self, format: &str, title: &str) -> Option<Vec<ExportedDocument>> {
        match format {
            "markdown" => Some(vec![ExportedDocument {
                path: "GRAPH_REPORT.md".to_owned(),
                content: self.to_markdown(title),
            }]),
            "wiki" => Some(self.to_wiki(title)),
            "obsidian" => Some(self.to_obsidian(title)),
            "html" => Some(vec![ExportedDocument {
                path: "graph.html".to_owned(),
                content: self.to_html(title),
            }]),
            "svg" => Some(vec![ExportedDocument {
                path: "graph.svg".to_owned(),
                content: self.to_svg(),
            }]),
            "graphml" => Some(vec![ExportedDocument {
                path: "graph.graphml".to_owned(),
                content: self.to_graphml(),
            }]),
            "neo4j" => Some(vec![ExportedDocument {
                path: "cypher.txt".to_owned(),
                content: self.to_neo4j(),
            }]),
            "callflow" => Some(vec![ExportedDocument {
                path: "callflow.md".to_owned(),
                content: self.to_callflow(),
            }]),
            _ => None,
        }
    }
}

fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::File => "file",
        NodeKind::Symbol(symbol) => symbol.as_str(),
    }
}

fn neo4j_label(value: impl Label) -> &'static str {
    value.label()
}

trait Label {
    fn label(self) -> &'static str;
}

impl Label for NodeKind {
    fn label(self) -> &'static str {
        kind_label(self)
    }
}

impl Label for EdgeKind {
    fn label(self) -> &'static str {
        match self {
            EdgeKind::Contains => "CONTAINS",
            EdgeKind::Calls => "CALLS",
            EdgeKind::Imports => "IMPORTS",
            EdgeKind::Inherits => "INHERITS",
            EdgeKind::Implements => "IMPLEMENTS",
            EdgeKind::ReferencesType => "REFERENCES_TYPE",
            EdgeKind::Accesses => "ACCESSES",
            EdgeKind::Writes => "WRITES",
            EdgeKind::Scip => "SCIP",
        }
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn escape_cypher(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn escape_yaml(value: &str) -> String {
    value.replace('\n', " ").replace(':', " -")
}

fn escape_mermaid(value: &str) -> String {
    value.replace('"', "'").replace('\n', " ")
}

fn sanitize_id(value: &str) -> String {
    value
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(char::from)
        .collect()
}
