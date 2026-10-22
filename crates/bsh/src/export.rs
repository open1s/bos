//! Session-log export: the Export command's real work.
//!
//! The reference's Export row downloads the Session log as a ZIP archive. bsh's
//! log is a directory (the manifest plus its Parquet segments), so the same row
//! writes those files into a ZIP the reader can keep.
//!
//! Entries are STORED (method 0), not deflated. The segments are already
//! compressed by the log writer, so a stored archive is both smaller to produce
//! and byte-exact; it also keeps the writer dependency-free, which matters for
//! a desktop binary that already links a large UI stack.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// One file that will become a ZIP entry.
struct Entry {
    name: String,
    data: Vec<u8>,
}

/// CRC-32 (IEEE 802.3), the checksum every ZIP reader verifies.
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 == 1 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
        }
        *slot = value;
    }
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc = table[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

/// Collect every regular file under `dir`, relative to it, name-ordered.
fn collect(dir: &Path, root: &Path, out: &mut Vec<Entry>) -> io::Result<()> {
    let mut children: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for path in children {
        if path.is_dir() {
            collect(&path, root, out)?;
        } else if path.is_file() {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let Some(name) = relative.to_str() else {
                continue;
            };
            out.push(Entry {
                name: name.replace('\\', "/"),
                data: fs::read(&path)?,
            });
        }
    }
    Ok(())
}

/// Write every file under `dir` into a store-only ZIP at `out`.
///
/// Returns the number of entries written. The archive is written beside the
/// target and renamed into place, so a failed export never leaves half a file
/// under the name the reader chose.
pub(crate) fn write_zip(dir: &Path, out: &Path) -> io::Result<usize> {
    let mut entries: Vec<Entry> = Vec::new();
    collect(dir, dir, &mut entries)?;
    let temporary = out.with_extension("zip.part");
    {
        let mut file = fs::File::create(&temporary)?;
        let mut offset: u32 = 0;
        let mut central: Vec<u8> = Vec::new();
        for entry in &entries {
            let name = entry.name.as_bytes();
            let crc = crc32(&entry.data);
            let size = entry.data.len() as u32;
            // Local file header, stored: version 20, flags 0, method 0, no
            // timestamp (a log's own files carry their real mtimes on disk).
            let mut local: Vec<u8> = Vec::with_capacity(30 + name.len());
            local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            local.extend_from_slice(&20u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(&crc.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(name);
            file.write_all(&local)?;
            file.write_all(&entry.data)?;
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
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
            offset += 30 + name.len() as u32 + size;
        }
        let central_offset = offset;
        file.write_all(&central)?;
        let mut end: Vec<u8> = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(central.len() as u32).to_le_bytes());
        end.extend_from_slice(&central_offset.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        file.write_all(&end)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, out).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    Ok(entries.len())
}

/// Write the audit as Markdown plus JSON beside the path the reader chose.
///
/// One save dialog names the pair: the chosen path receives the Markdown unless
/// it already carries a .json extension, in which case it receives the JSON and
/// the Markdown takes the sibling .md. Both files are written beside the target
/// and renamed into place, so a failed export never leaves half a file under the
/// name the reader chose.
pub(crate) fn write_audit(
    view: &crate::audit::AuditView,
    lang: crate::i18n::Lang,
    out: &Path,
) -> io::Result<(PathBuf, PathBuf)> {
    let chose_json = out.extension().and_then(|ext| ext.to_str()) == Some("json");
    let (markdown_path, json_path) = if chose_json {
        (out.with_extension("md"), out.to_path_buf())
    } else {
        (out.to_path_buf(), out.with_extension("json"))
    };
    write_atomic(&markdown_path, view.to_markdown(lang).as_bytes())?;
    write_atomic(&json_path, view.to_json().as_bytes())?;
    Ok((markdown_path, json_path))
}

/// Write one file through a temporary beside it and rename it into place.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let suffix = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("out");
    let temporary = path.with_extension(format!("{suffix}.part"));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::{crc32, write_audit, write_zip};

    /// A store-only archive a ZIP reader can walk: local headers, the entries'
    /// bytes, then the central directory and its end record.
    #[test]
    fn an_archive_carries_every_file_and_its_crc() {
        let root = std::env::temp_dir().join(format!("bsh-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("segments")).expect("a scratch log");
        std::fs::write(root.join("manifest.json"), b"{\"rows\":2}").expect("the manifest");
        std::fs::write(root.join("segments/one.parquet"), b"PAR1-one").expect("a segment");
        let out = root.join("session.zip");
        let written = write_zip(&root, &out).expect("the archive");
        assert_eq!(written, 2, "one entry per file");
        let bytes = std::fs::read(&out).expect("the archive bytes");
        assert_eq!(&bytes[..4], b"PK\x03\x04", "a local file header opens it");
        assert_eq!(&bytes[bytes.len() - 22..bytes.len() - 18], b"PK\x05\x06");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("manifest.json"), "the manifest is an entry");
        assert!(
            text.contains("segments/one.parquet"),
            "a segment keeps its relative path"
        );
        // The first local entry's CRC covers exactly its bytes.
        let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[28], bytes[29]]) as usize;
        let crc = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
        let size = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]) as usize;
        let start = 30 + name_len + extra_len;
        assert_eq!(size, 10, "manifest.json is the first entry");
        assert_eq!(
            crc,
            crc32(&bytes[start..start + size]),
            "the manifest's CRC"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The checksum is the standard one, so any reader agrees with it.
    #[test]
    fn the_checksum_is_the_zip_crc() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    /// The audit export writes BOTH files through the chosen path, and the JSON
    /// is valid with one object per call.
    #[test]
    fn the_audit_export_writes_markdown_and_json() {
        let root = std::env::temp_dir().join(format!("bsh-audit-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let view = crate::audit::AuditView {
            totals: crate::audit::AuditTotals {
                turns: 1,
                calls: 2,
                files_read: 1,
                files_written: 0,
                commands: 1,
                refusals: 1,
                succeeded: 2,
                failed: 0,
                pending: 0,
                duration_ms: 1_200,
            },
            policy: crate::audit::AuditPolicy::default(),
            turns: vec![crate::audit::AuditTurn {
                index: 1,
                message: 0,
                started_at: Some(1_000),
                duration_ms: 1_200,
                calls: vec![
                    crate::audit::AuditCall {
                        tool: "read_file".to_string(),
                        category: crate::audit::AuditCategory::FileRead,
                        target: "a.md".to_string(),
                        outcome: crate::audit::AuditOutcome::Ok,
                        ms: Some(4),
                        preview: "body".to_string(),
                        message: 0,
                        tool_index: 0,
                    },
                    crate::audit::AuditCall {
                        tool: "bash".to_string(),
                        category: crate::audit::AuditCategory::Shell,
                        target: "ls".to_string(),
                        outcome: crate::audit::AuditOutcome::Ok,
                        ms: Some(9),
                        preview: "a.md".to_string(),
                        message: 0,
                        tool_index: 1,
                    },
                ],
                refusals: vec![crate::audit::AuditRefusal {
                    at_ms: 1_500,
                    tool: "write_file".to_string(),
                    target: "/etc/x".to_string(),
                    permission: "read-only".to_string(),
                    action: "write".to_string(),
                    rule: Some("deny-outside".to_string()),
                    reason: None,
                }],
            }],
        };
        let chosen = root.join("audit.md");
        let (markdown_path, json_path) =
            write_audit(&view, crate::i18n::Lang::En, &chosen).expect("the audit export");
        assert_eq!(markdown_path, chosen);
        assert_eq!(json_path, root.join("audit.json"));
        let markdown = std::fs::read_to_string(&markdown_path).expect("the Markdown");
        assert!(markdown.contains("read_file"));
        assert!(markdown.contains("deny-outside"));
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).expect("the JSON"))
                .expect("valid JSON");
        let calls: usize = json["turns"]
            .as_array()
            .expect("turns")
            .iter()
            .map(|turn| turn["calls"].as_array().map_or(0, Vec::len))
            .sum();
        assert_eq!(calls, 2, "one JSON object per call");
        // Choosing a .json path flips the pair rather than overwriting it.
        let chosen_json = root.join("second.json");
        let (md, json) = write_audit(&view, crate::i18n::Lang::En, &chosen_json).expect("the pair");
        assert_eq!(md, root.join("second.md"));
        assert_eq!(json, chosen_json);
        let _ = std::fs::remove_dir_all(&root);
    }
}
