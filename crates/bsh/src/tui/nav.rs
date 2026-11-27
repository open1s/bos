//! The navigation rail: the concept's first column.
//!
//! The mock's left column is navigation, not a list of chats: a `Workspace`
//! section naming every destination the workspace has, a `Panels` section for
//! the documents that open over it, the `Views` samples, and the workspace
//! version and ready state at the foot.
//!
//! Each destination is a name the client already answers to - the same command
//! a reader can type - so the rail is a face on the client's own vocabulary
//! rather than a second navigation system beside it. Sessions remain available
//! through the conversations destination and its keyboard shortcuts.
//!
//! Like [`super::board`], this is data in and rows out, so a frame can be
//! checked without a terminal.

use super::app::{cells, truncate};
use super::skin::Tone;

/// One row of runs: a piece of text and the ink it is painted in.
pub(crate) type Row = Vec<(String, Tone)>;

/// Every destination receives the same two-cell icon slot, even when its
/// Unicode glyph occupies only one terminal cell.
const ICON_SLOT_WIDTH: usize = 2;

/// One destination the rail offers.
pub(crate) struct Item {
    /// The glyph the concept draws in front of the name.
    pub(crate) glyph: char,
    /// The destination's name, as the concept writes it.
    pub(crate) name: &'static str,
    /// The letter the rail prints at its right edge and the chord it stands
    /// for: the composer owns the plain letters, so the chord is `Alt` plus it.
    pub(crate) key: char,
    /// The command opening this destination: the rail runs what a reader could
    /// have typed, so there is one navigation path with two faces.
    pub(crate) command: &'static str,
}

/// The workspace destinations, in the concept's own order.
pub(crate) const WORKSPACE: [Item; 7] = [
    Item {
        glyph: '▣',
        name: "Conversations",
        key: 'C',
        command: "/sessions",
    },
    Item {
        glyph: '▤',
        name: "Jobs",
        key: 'J',
        command: "/tasks",
    },
    Item {
        glyph: '♙',
        name: "Agents",
        key: 'A',
        command: "/subagents",
    },
    Item {
        glyph: '⚒',
        name: "Tools",
        key: 'T',
        command: "/mcp",
    },
    Item {
        glyph: '⬡',
        name: "Capabilities",
        key: 'P',
        command: "/capabilities",
    },
    Item {
        glyph: '◷',
        name: "Activity",
        key: 'L',
        command: "/activity",
    },
    Item {
        glyph: '▧',
        name: "Audit",
        key: 'U',
        command: "/audit",
    },
];

/// The panels: documents that take the body rather than a destination of their
/// own.
pub(crate) const PANELS: [Item; 3] = [
    Item {
        glyph: '▦',
        name: "Inspector",
        key: 'I',
        command: "/detail",
    },
    Item {
        glyph: '▱',
        name: "Files",
        key: 'F',
        command: "/files",
    },
    Item {
        glyph: '⚙',
        name: "Settings",
        key: 'S',
        command: "/settings",
    },
];

/// One row of the command palette.
///
/// The palette is the rail made typeable: a frame narrower than two columns has
/// no rail to walk, so its destinations have to be reachable by name. Each row
/// carries the line it runs, which is the same line a reader could have typed -
/// one navigation path with two faces, the way the rail itself is a face on the
/// commands rather than a second system beside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The name the row leads with, and what the filter ranks.
    pub(crate) name: &'static str,
    /// The spelling the row runs, drawn beside the name: a reader who left
    /// through the palette still learns the command they picked.
    pub(crate) command: &'static str,
}

/// The argument-free actions the rail has no row for.
///
/// A command that needs an argument is left out rather than listed and then
/// answered with its own usage line: the palette says where a reader can go
/// from here, and a row that only refuses is noise in a list this short. The
/// order is the order a reader reaches for them.
const ACTIONS: [Entry; 20] = [
    Entry {
        name: "New conversation",
        command: "/new",
    },
    Entry {
        name: "Help",
        command: "/help",
    },
    Entry {
        name: "Model",
        command: "/model",
    },
    Entry {
        name: "Preset",
        command: "/preset",
    },
    Entry {
        name: "Approval mode",
        command: "/permission",
    },
    Entry {
        name: "Plan",
        command: "/plan",
    },
    Entry {
        name: "Goal",
        command: "/goal",
    },
    Entry {
        name: "Compact history",
        command: "/compact",
    },
    Entry {
        name: "Usage",
        command: "/usage",
    },
    Entry {
        name: "Open a terminal",
        command: "/terminal",
    },
    Entry {
        name: "Review changes",
        command: "/review",
    },
    Entry {
        name: "Diff",
        command: "/diff",
    },
    Entry {
        name: "Skills",
        command: "/skills",
    },
    Entry {
        name: "Extensions",
        command: "/extensions",
    },
    Entry {
        name: "Memory",
        command: "/memory",
    },
    Entry {
        name: "Federation",
        command: "/federation",
    },
    Entry {
        name: "Browser",
        command: "/browser",
    },
    Entry {
        name: "Questions",
        command: "/questions",
    },
    Entry {
        name: "The bin",
        command: "/trash",
    },
    Entry {
        name: "Queue",
        command: "/queue",
    },
];

/// Every row the palette offers: the rail's destinations and panels in the
/// concept's own order, then the actions the rail leaves to typing.
pub(crate) fn entries() -> Vec<Entry> {
    WORKSPACE
        .iter()
        .chain(PANELS.iter())
        .map(|item| Entry {
            name: item.name,
            command: item.command,
        })
        .chain(ACTIONS)
        .collect()
}

/// What a rail row names, when it names anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hit {
    /// Expand or collapse the rail.
    Toggle,
    /// A workspace destination, by index into [`WORKSPACE`].
    Workspace(usize),
    /// A panel, by index into [`PANELS`].
    Panel(usize),
}

/// Everything the rail paints, gathered by the app.
pub(crate) struct Nav {
    /// The destination the frame is on, when it is on one.
    pub(crate) active: Option<Hit>,
    /// The workspace's verdict for the foot of the column.
    pub(crate) status: (String, Tone),
    /// The client's version, shown at the foot of the rail.
    pub(crate) version: &'static str,
    /// The selected view, highlighted in the Views section.
    pub(crate) view: &'static str,
    /// Whether the rail is an icon-only strip.
    pub(crate) collapsed: bool,
}

/// The rows the rail paints on a column `width` cells wide and `rows` tall,
/// with the name each row carries.
///
/// Exactly `rows` rows come back, and version and status occupy its last two
/// rows. Sections that do not fit are omitted before destinations.
pub(crate) fn lines(nav: &Nav, width: usize, rows: usize) -> (Vec<Row>, Vec<Option<Hit>>) {
    if nav.collapsed {
        return compact_lines(nav, width, rows);
    }
    let mut painted: Vec<(Row, Option<Hit>)> = Vec::new();
    painted.push((
        vec![
            (pad(" Workspace", width.saturating_sub(2)), Tone::Muted),
            (" ‹".to_string(), Tone::Accent),
        ],
        Some(Hit::Toggle),
    ));
    for (index, item) in WORKSPACE.iter().enumerate() {
        painted.push((
            item_row(item, width, nav.active == Some(Hit::Workspace(index))),
            Some(Hit::Workspace(index)),
        ));
    }
    // The two optional blocks, kept only when the destinations and the blocks
    // themselves fit under one column of the size the frame handed us.
    let heading = 1usize;
    let panels_block = heading + PANELS.len() + 1;
    let views_block = heading + 5 + 1;
    // Reserve the final two rows for version and readiness.
    let budget = rows.saturating_sub(2);
    let with_panels = painted.len() + panels_block <= budget;
    let with_views = with_panels && painted.len() + panels_block + views_block <= budget;
    if with_panels {
        painted.push((section_rule(width), None));
        painted.push((label("Panels", width), None));
        for (index, item) in PANELS.iter().enumerate() {
            painted.push((
                item_row(item, width, nav.active == Some(Hit::Panel(index))),
                Some(Hit::Panel(index)),
            ));
        }
    }
    if with_views {
        painted.push((section_rule(width), None));
        painted.push((label("Views", width), None));
        for (name, key) in [
            ("Adaptive", '1'),
            ("Sidebar", '2'),
            ("Modal", '3'),
            ("Inline", '4'),
            ("Ratatui", '5'),
        ] {
            let selected = name == nav.view;
            let tone = if selected { Tone::Ok } else { Tone::Body };
            let marker = if selected { " ● " } else { "   " };
            let text = format!("{marker}{name}");
            let gap = width.saturating_sub(cells(&text) + 1).max(1);
            painted.push((
                vec![
                    (text, tone),
                    (" ".repeat(gap), Tone::Body),
                    (key.to_string(), Tone::Muted),
                ],
                None,
            ));
        }
    }
    // The column's foot: version and ready state, with the same fixed position
    // on every frame.
    let foot = rows.saturating_sub(2);
    painted.truncate(foot);
    while painted.len() < foot {
        painted.push((blank(width), None));
    }
    painted.push((
        vec![(pad(&format!(" BSH v{}", nav.version), width), Tone::Muted)],
        None,
    ));
    painted.push((
        vec![(pad(&format!(" {}", nav.status.0), width), nav.status.1)],
        None,
    ));
    let mut hits: Vec<Option<Hit>> = painted.iter().map(|(_, hit)| *hit).collect();
    let rows_out: Vec<Row> = painted.into_iter().map(|(row, _)| row).collect();
    hits.truncate(rows_out.len());
    (rows_out, hits)
}

/// The collapsed rail keeps every destination as a clickable icon and puts an
/// expand control at its top edge.
fn compact_lines(nav: &Nav, width: usize, rows: usize) -> (Vec<Row>, Vec<Option<Hit>>) {
    let mut painted = vec![(vec![(pad(" ›", width), Tone::Accent)], Some(Hit::Toggle))];
    for (index, item) in WORKSPACE.iter().enumerate() {
        painted.push((
            compact_item(item, width, nav.active == Some(Hit::Workspace(index))),
            Some(Hit::Workspace(index)),
        ));
    }
    for (index, item) in PANELS.iter().enumerate() {
        painted.push((
            compact_item(item, width, nav.active == Some(Hit::Panel(index))),
            Some(Hit::Panel(index)),
        ));
    }
    painted.truncate(rows);
    while painted.len() < rows {
        painted.push((blank(width), None));
    }
    let hits = painted.iter().map(|(_, hit)| *hit).collect();
    let rows = painted.into_iter().map(|(row, _)| row).collect();
    (rows, hits)
}

/// One icon destination in the collapsed rail.
fn compact_item(item: &Item, width: usize, active: bool) -> Row {
    let icon = if active {
        Tone::NavIconActive
    } else {
        Tone::NavIcon
    };
    let fill = if active { Tone::Select } else { Tone::Body };
    let glyph = item.glyph.to_string();
    vec![
        (" ".to_string(), fill),
        (icon_slot(&glyph), icon),
        (" ".repeat(width.saturating_sub(1 + ICON_SLOT_WIDTH)), fill),
    ]
}

/// A section's name: the concept sets these in the muted ink, uppercase.
fn label(word: &str, width: usize) -> Row {
    vec![(pad(&format!(" {word}"), width), Tone::Muted)]
}

/// A section divider within the rail, aligned with the vertical frame rules.
fn section_rule(width: usize) -> Row {
    vec![("─".repeat(width), Tone::Muted)]
}

/// One destination: its glyph and name on the left, its chord at the right
/// edge - the row the reader is on drawn on the concept's raised fill.
fn item_row(item: &Item, width: usize, active: bool) -> Row {
    let fill = if active { Tone::Select } else { Tone::Body };
    let icon = if active {
        Tone::NavIconActive
    } else {
        Tone::NavIcon
    };
    let glyph = item.glyph.to_string();
    let key = item.key.to_string();
    let key_width = cells(&key);
    // Reserve the leading cell, the icon slot, one cell of air and the chord
    // before truncating the label, so a narrow rail can never push the chord
    // past the column's right edge.
    let reserved = 1 + ICON_SLOT_WIDTH + 1 + key_width;
    let label_max = width.saturating_sub(reserved);
    let label = truncate(&format!(" {}", item.name), label_max);
    let used = 1 + ICON_SLOT_WIDTH + cells(&label);
    let gap = width.saturating_sub(used + key_width).max(1);
    let trailing = width.saturating_sub(used + gap + key_width);
    let mut row: Row = vec![
        (" ".to_string(), fill),
        (icon_slot(&glyph), icon),
        (label, fill),
        (" ".repeat(gap), fill),
        (key, if active { Tone::Select } else { Tone::Muted }),
    ];
    if trailing > 0 {
        row.push((" ".repeat(trailing), fill));
    }
    row
}

/// Place each glyph in the same terminal-cell slot without letting wide
/// Unicode symbols change the label alignment.
fn icon_slot(glyph: &str) -> String {
    let glyph = truncate(glyph, ICON_SLOT_WIDTH);
    let used = cells(&glyph);
    format!(
        "{glyph}{}",
        " ".repeat(ICON_SLOT_WIDTH.saturating_sub(used))
    )
}

/// A blank row, painted to the column's width so the rule beside it keeps its
/// place.
fn blank(width: usize) -> Row {
    vec![(" ".repeat(width), Tone::Body)]
}

/// `text` filled to `width` cells.
fn pad(text: &str, width: usize) -> String {
    let text = truncate(text, width);
    let mut padded = text;
    let used = cells(&padded);
    if used < width {
        padded.push_str(&" ".repeat(width - used));
    }
    padded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nav() -> Nav {
        Nav {
            active: Some(Hit::Workspace(0)),
            status: ("● BSH · Ready".to_string(), Tone::Ok),
            version: "0.1.0",
            view: "Adaptive",
            collapsed: false,
        }
    }

    fn text(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|runs| {
                runs.iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn the_rail_names_every_destination_the_concept_names() {
        let (rows, hits) = lines(&nav(), 30, 30);
        let painted = text(&rows);
        for (index, item) in WORKSPACE.iter().enumerate() {
            let row = painted
                .iter()
                .find(|row| row.contains(item.name))
                .unwrap_or_else(|| panic!("{} is painted", item.name));
            assert!(
                row.contains(item.key),
                "{} carries its chord: {row:?}",
                item.name
            );
            assert_eq!(
                hits[painted.iter().position(|r| r == row).unwrap()],
                Some(Hit::Workspace(index))
            );
        }
        assert!(painted.iter().any(|row| row.contains("Workspace")));
        assert!(painted.iter().any(|row| row.contains("Panels")));
        assert!(painted.iter().any(|row| row.contains("Views")));
        assert!(painted.iter().any(|row| row.contains("Adaptive")));
        for view in ["Sidebar", "Modal", "Inline", "Ratatui"] {
            assert!(painted.iter().any(|row| row.contains(view)), "{view}");
        }
        assert!(painted.iter().any(|row| row.contains("BSH v0.1.0")));
    }

    #[test]
    fn the_open_destination_wears_the_selection_fill() {
        let (rows, _) = lines(&nav(), 30, 30);
        let active = &rows[1];
        assert!(
            active.iter().any(|(_, tone)| *tone == Tone::Select),
            "the active destination is filled: {active:?}"
        );
        let other = &rows[2];
        assert!(other.iter().all(|(_, tone)| *tone != Tone::Select));
    }

    #[test]
    fn every_destination_a_row_paints_can_be_named() {
        let (_, hits) = lines(&nav(), 30, 30);
        assert_eq!(hits[1], Some(Hit::Workspace(0)));
        // The panels sit under their own heading, whose row names nothing.
        let panels_heading = hits
            .iter()
            .position(|hit| *hit == Some(Hit::Panel(0)))
            .expect("the panels are painted");
        assert_eq!(hits[panels_heading - 1], None, "a heading names nothing");
    }

    #[test]
    fn the_status_is_the_columns_last_row() {
        for rows in [12usize, 20, 30, 60] {
            let (painted, _) = lines(&nav(), 30, rows);
            assert_eq!(painted.len(), rows, "the column keeps its height");
            let last = text(&painted).pop().unwrap();
            assert!(
                last.contains("BSH · Ready"),
                "the foot is the status: {last:?}"
            );
        }
    }

    #[test]
    fn a_short_frame_drops_views_before_the_destinations() {
        // A column too short for everything keeps destinations and panels,
        // then drops the design samples to preserve navigation.
        let (rows, hits) = lines(&nav(), 30, 14);
        let painted = text(&rows);
        assert!(painted.iter().any(|row| row.contains("Conversations")));
        assert!(!painted.iter().any(|row| row.contains("Views")));
        assert!(
            hits.iter()
                .flatten()
                .all(|hit| matches!(hit, Hit::Toggle | Hit::Workspace(_) | Hit::Panel(_)))
        );
    }

    #[test]
    fn a_collapsed_rail_keeps_clickable_icons_and_an_expand_control() {
        let mut nav = nav();
        nav.collapsed = true;
        let (rows, hits) = lines(&nav, 6, 16);
        assert_eq!(hits[0], Some(Hit::Toggle));
        for (index, item) in WORKSPACE.iter().enumerate() {
            assert!(
                rows[index + 1]
                    .iter()
                    .any(|(text, _)| text.contains(item.glyph))
            );
            assert_eq!(hits[index + 1], Some(Hit::Workspace(index)));
        }
        for index in 0..PANELS.len() {
            assert_eq!(hits[index + 1 + WORKSPACE.len()], Some(Hit::Panel(index)));
        }
        assert!(
            rows.iter()
                .all(|row| cells(&text(std::slice::from_ref(row))[0]) <= 6)
        );
    }

    #[test]
    fn every_row_stays_inside_the_column() {
        for width in [18usize, 22, 30] {
            for rows in [6usize, 14, 24, 48] {
                let (painted, hits) = lines(&nav(), width, rows);
                assert_eq!(painted.len(), hits.len());
                for row in &painted {
                    let used: usize = row.iter().map(|(text, _)| cells(text)).sum();
                    assert!(used <= width, "row over {width} cells: {row:?}");
                }
            }
        }
    }

    #[test]
    fn the_chord_a_row_advertises_is_the_chord_the_client_binds() {
        // The rail prints a letter and the client binds Alt plus it: the two
        // must come from the same table or the hint teaches the wrong key.
        let keys: Vec<char> = WORKSPACE
            .iter()
            .chain(PANELS.iter())
            .map(|item| item.key)
            .collect();
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "no chord is bound twice");
        for item in WORKSPACE.iter().chain(PANELS.iter()) {
            assert!(
                item.command.starts_with('/'),
                "{} opens a command",
                item.name
            );
        }
    }
}
