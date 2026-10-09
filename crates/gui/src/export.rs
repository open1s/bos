//! Keep a chat as a file: a plain ZIP the reader can open anywhere.
//!
//! The GUI could shell out to a compressor or take a dependency, but neither is
//! available offline and neither is needed: entries are written **stored**, so
//! the archive is a header plus the bytes themselves. That also means an export
//! is inspectable with the most basic tools, which is the point of escaping a
//! database in the first place.
//!
//! Limits, stated rather than implied: ZIP64 is not implemented, so an archive
//! is capped at 4 GiB and 65535 entries, and timestamps are DOS format (two
//! second resolution, UTC, no time zone).

use std::path::{Path, PathBuf};

use crate::session::{ChatMessage, Role, SessionRecord};

/// One file inside the archive.
struct Entry {
    /// Slash-separated name as stored in the archive.
    name: String,
    /// The bytes, written uncompressed.
    data: Vec<u8>,
}

/// CRC-32 table (reflected polynomial 0xEDB88320), built at compile time so the
/// writer needs no initialization and no dependency.
const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// Shared CRC-32 table.
static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 of `bytes`, as ZIP stores it.
///
/// Exposed so a test can check it against the published check value for
/// `"123456789"` rather than against this same function.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &byte in bytes {
        c = CRC_TABLE[((c ^ u32::from(byte)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

/// DOS date and time fields for a unix timestamp (UTC, even seconds).
///
/// Returned so tests can assert the fields rather than a packed constant.
pub(crate) fn dos_stamp(unix: u64) -> (u16, u16) {
    let secs = unix as i64;
    let days = secs.div_euclid(86_400);
    // A day holds 86_400 seconds, so this must not be a u16: 80_000 would
    // truncate and the archive would carry a wrong timestamp.
    let rem = secs.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let date = (((year - 1980).clamp(0, 127) as u16) << 9) | ((month as u16) << 5) | day as u16;
    let time = (((rem / 3600) as u16) << 11)
        | ((((rem % 3600) / 60) as u16) << 5)
        | ((rem % 60 / 2) as u16);
    (date, time)
}

/// Assemble a stored archive from `entries`.
fn build_zip(entries: &[Entry], unix: u64) -> Vec<u8> {
    let (date, time) = dos_stamp(unix);
    let mut body: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for entry in entries {
        let offset = body.len() as u32;
        let crc = crc32(&entry.data);
        let size = entry.data.len() as u32;
        let name = entry.name.as_bytes();
        body.extend_from_slice(b"PK\x03\x04");
        body.extend_from_slice(&20u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&time.to_le_bytes());
        body.extend_from_slice(&date.to_le_bytes());
        body.extend_from_slice(&crc.to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(name);
        body.extend_from_slice(&entry.data);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&time.to_le_bytes());
        central.extend_from_slice(&date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let central_offset = body.len() as u32;
    let central_size = central.len() as u32;
    body.extend_from_slice(&central);
    body.extend_from_slice(b"PK\x05\x06");
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    body.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    body.extend_from_slice(&central_size.to_le_bytes());
    body.extend_from_slice(&central_offset.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body
}

/// A chat as Markdown: the same turns, in order, without the JSON noise.
pub(crate) fn transcript_markdown(record: &SessionRecord) -> String {
    let title = record.title.trim();
    let mut out = format!(
        "# {}\n\n{}\n\n",
        if title.is_empty() {
            "Untitled chat"
        } else {
            title
        },
        if record.workspace.trim().is_empty() {
            "No workspace recorded.".to_string()
        } else {
            format!("Workspace: `{}`.", record.workspace.trim())
        }
    );
    // Folded turns come first because they are older than what replaced them.
    for message in record.archived.iter().chain(record.messages.iter()) {
        out.push_str(&render_message(message));
    }
    out
}

/// One turn as Markdown.
fn render_message(message: &ChatMessage) -> String {
    let mut out = match message.role {
        Role::User => "## User\n\n".to_string(),
        Role::Assistant => "## Assistant\n\n".to_string(),
    };
    if !message.reasoning.trim().is_empty() {
        out.push_str("> Reasoning:\n");
        for line in message.reasoning.trim().lines() {
            out.push_str(&format!("> {line}\n"));
        }
        out.push('\n');
    }
    if !message.text.trim().is_empty() {
        out.push_str(message.text.trim_end());
        out.push_str("\n\n");
    }
    for tool in &message.tools {
        out.push_str(&format!("- tool `{}`\n", tool.name));
    }
    if !message.tools.is_empty() {
        out.push('\n');
    }
    if let Some(error) = &message.error {
        out.push_str(&format!("Failed: {error}\n\n"));
    }
    out
}

/// The archive bytes for one chat.
pub(crate) fn archive_bytes(record: &SessionRecord) -> Result<Vec<u8>, String> {
    let json =
        serde_json::to_vec_pretty(record).map_err(|e| format!("cannot serialize chat: {e}"))?;
    let readme = format!(
        "Chat exported from BOS.\n\n\
         chat.json holds the record exactly as the store keeps it.\n\
         chat.md is the same conversation rendered as Markdown.\n\
         Entries are stored, not compressed, so this archive is readable with any tool.\n\n\
         Exported at unix {}.\n",
        record.updated_at
    );
    let entries = vec![
        Entry {
            name: "README.txt".to_string(),
            data: readme.into_bytes(),
        },
        Entry {
            name: "chat.json".to_string(),
            data: json,
        },
        Entry {
            name: "chat.md".to_string(),
            data: transcript_markdown(record).into_bytes(),
        },
    ];
    Ok(build_zip(&entries, record.updated_at))
}

/// Write one chat beside the session store and return the path written.
pub(crate) fn export_chat(root: &Path, record: &SessionRecord) -> Result<PathBuf, String> {
    let bytes = archive_bytes(record)?;
    let dir = root.join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.zip", record.id));
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ToolEvent;

    /// Parse the archive the way a reader would, so the assertions describe the
    /// file format instead of the writer that produced it.
    struct Read {
        name: String,
        crc: u32,
        data: Vec<u8>,
    }

    fn read_archive(bytes: &[u8]) -> Vec<Read> {
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + 4 <= bytes.len() && &bytes[at..at + 4] == b"PK\x03\x04" {
            let name_len = u16::from_le_bytes([bytes[at + 26], bytes[at + 27]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[at + 28], bytes[at + 29]]) as usize;
            let crc = u32::from_le_bytes([
                bytes[at + 14],
                bytes[at + 15],
                bytes[at + 16],
                bytes[at + 17],
            ]);
            let size = u32::from_le_bytes([
                bytes[at + 18],
                bytes[at + 19],
                bytes[at + 20],
                bytes[at + 21],
            ]) as usize;
            let name_at = at + 30;
            let name = String::from_utf8(bytes[name_at..name_at + name_len].to_vec()).unwrap();
            let data_at = name_at + name_len + extra_len;
            let data = bytes[data_at..data_at + size].to_vec();
            out.push(Read { name, crc, data });
            at = data_at + size;
        }
        assert_eq!(
            &bytes[at..at + 4],
            b"PK\x01\x02",
            "central directory follows"
        );
        out
    }

    #[test]
    fn crc32_matches_the_published_check_value() {
        // A known answer, not a mirror of this implementation.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn dos_time_converts_a_known_instant() {
        // 2023-11-14T22:13:20Z.
        let (date, time) = dos_stamp(1_700_000_000);
        assert_eq!(date, ((2023 - 1980) << 9) | (11 << 5) | 14);
        assert_eq!(time, (22 << 11) | (13 << 5) | (20 / 2));
    }

    #[test]
    fn an_archive_round_trips_through_a_reader() {
        let mut record = SessionRecord::new();
        record.title = "Export me".to_string();
        record.messages.push(ChatMessage::user("first ask"));
        let mut answer = ChatMessage::assistant();
        answer.text = "the reply".to_string();
        answer.tools.push(ToolEvent {
            name: "read".into(),
            args: "{}".into(),
            output: None,
            ms: Some(2),
        });
        record.messages.push(answer);
        record.archived.push(ChatMessage::user("older ask"));

        let bytes = archive_bytes(&record).expect("archive");
        let read = read_archive(&bytes);
        let names: Vec<&str> = read.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["README.txt", "chat.json", "chat.md"]);
        for entry in &read {
            assert_eq!(crc32(&entry.data), entry.crc, "{} checksum", entry.name);
        }
        let json = String::from_utf8(read[1].data.clone()).unwrap();
        assert!(json.contains("\"first ask\""), "the record round-trips");
        assert!(
            json.contains("\"older ask\""),
            "folded turns are exported too"
        );
        let md = String::from_utf8(read[2].data.clone()).unwrap();
        assert!(md.starts_with("# Export me"));
        assert!(md.contains("## User"));
        assert!(md.contains("## Assistant"));
        // Folded turns come first, because they are older.
        assert!(md.find("older ask").unwrap() < md.find("first ask").unwrap());
        assert!(md.contains("- tool `read`"));
        // The end record names every entry.
        let eocd = &bytes[bytes.len() - 22..];
        assert_eq!(&eocd[..4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([eocd[10], eocd[11]]), 3);
    }

    #[test]
    fn a_chat_with_nothing_in_it_still_exports() {
        let record = SessionRecord::new();
        let bytes = archive_bytes(&record).expect("archive");
        assert_eq!(read_archive(&bytes).len(), 3);
    }

    #[test]
    fn exporting_writes_where_it_says_it_wrote() {
        let mut record = SessionRecord::new();
        record.messages.push(ChatMessage::user("hello"));
        // A guard script sets BOS_EXPORT_KEEP to a directory and reads the path
        // back, so the archive can be validated by an independent reader.
        let keep = std::env::var_os("BOS_EXPORT_KEEP");
        let root = match &keep {
            Some(dir) => PathBuf::from(dir),
            None => std::env::temp_dir().join(format!("bos-export-test-{}", std::process::id())),
        };
        let path = export_chat(&root, &record).expect("export");
        assert!(path.starts_with(&root), "kept inside the root it was given");
        let written = std::fs::read(&path).expect("read back");
        assert_eq!(written, archive_bytes(&record).expect("archive"));
        if keep.is_some() {
            println!("BOS_EXPORT_PATH={}", path.display());
        } else {
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
