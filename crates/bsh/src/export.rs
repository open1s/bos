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

#[cfg(test)]
mod tests {
    use super::{crc32, write_zip};

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
}
