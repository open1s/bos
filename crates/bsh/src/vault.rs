//! The admin-key vault: one encrypted file of confidential values.
//!
//! The file lives at the bsh data directory as '.vault.bin', and is the only place a value
//! the settings file must not carry can live. Everything is sealed into one
//! authenticated document: there is no plaintext fallback, no per-entry file and
//! no environment variable in the path.
//!
//! Beside it sits '.vault.access': a 0600 file holding the derived key, written
//! when the vault is created or unlocked. It is the app's own permission to read
//! provider entries at launch without asking for the admin passphrase. The
//! passphrase is never stored; deleting the access file revokes the app's
//! passwordless access until the vault is unlocked again.
//!
//! # Format
//!
//! The file is a header followed by one AEAD ciphertext:
//!
//! | offset | bytes | meaning |
//! |--------|-------|---------|
//! | 0      | 4     | magic 'BSHV' |
//! | 4      | 1     | format version, 1 |
//! | 5      | 16    | Argon2id salt, random per file |
//! | 21     | 24    | XChaCha20-Poly1305 nonce, random per write |
//! | 45     | ..    | ciphertext, then the 16-byte Poly1305 tag |
//!
//! The associated data is the secret-independent header from offset 4 through
//! offset 20: the version byte and the salt. It binds the ciphertext to the
//! exact key-derivation parameters the file was written with, so a file whose
//! salt was swapped fails the tag check instead of decrypting under a key the
//! reader never derived.
//!
//! The plaintext is a length-prefixed table of name, value and created (unix
//! seconds) records; see 'Unlocked::serialize'. Names namespaced as
//! provider:ID:api_key carry provider credentials; any other
//! non-empty name is legal and is used for values with no provider behind them.
//!
//! # Cryptography
//!
//! Argon2id (RFC 9106), version 0x13, with m_cost = 65536 KiB (64 MiB),
//! t_cost = 3 passes, p_cost = 1 lane and a 32-byte output. The salt is 16
//! random bytes generated from the operating system on creation and kept for the
//! file's life. The derived key is held in a zeroize::Zeroizing allocation and
//! used for the AEAD. It is also written to the 0600 '.vault.access' sidecar so
//! the app can reopen its provider entries without the admin passphrase; the
//! passphrase itself is never written. 'Vault::lock' only drops the ADMIN view:
//! provider resolution keeps working while the Vault pane is locked.
//!
//! Sealing uses XChaCha20-Poly1305 with a fresh 24-byte nonce per write. A write
//! re-encrypts the whole table, so the nonce is never reused even when the
//! entries did not change.
//!
//! # Atomicity and permissions
//!
//! A write goes to a temporary file in the same directory with mode 0600, is
//! flushed, and is then renamed over '.vault.bin', so a reader never observes a
//! half-written document and an interrupted write leaves the previous file
//! intact.
//!
//! # Errors
//!
//! Every failure is explicit: 'VaultError::NoVault', 'VaultError::Locked',
//! 'VaultError::WrongKey' (an AEAD tag failure, including a wrong passphrase),
//! 'VaultError::Io', 'VaultError::Parse', and the narrow argument refusals
//! 'VaultError::EmptyPassphrase', 'VaultError::InvalidEntry' and
//! 'VaultError::NotFound'. No error, and no Debug implementation in this
//! module, ever contains a value or a passphrase.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::rngs::SysRng;
use rand::TryRng;
use zeroize::Zeroizing;

/// The four bytes a vault file starts with.
const MAGIC: &[u8; 4] = b"BSHV";
/// The only format version this build writes or reads.
const VERSION: u8 = 1;
/// The salt length, in bytes.
const SALT_LEN: usize = 16;
/// The XChaCha20-Poly1305 nonce length, in bytes.
const NONCE_LEN: usize = 24;
/// The derived key length, in bytes.
const KEY_LEN: usize = 32;
/// The Poly1305 tag length, in bytes.
const TAG_LEN: usize = 16;
/// Argon2id memory cost, in KiB: 64 MiB, the documented floor.
const M_COST_KIB: u32 = 64 * 1024;
/// Argon2id pass count, the documented floor.
const T_COST: u32 = 3;
/// Argon2id lane count.
const P_COST: u32 = 1;
/// magic + version + salt: the offset the nonce begins at.
const HEADER_LEN: usize = MAGIC.len() + 1 + SALT_LEN;
/// The 0600 sidecar holding the app-access key, beside the vault file.
const ACCESS_FILE: &str = ".vault.access";

/// The vault's explicit failure, never carrying a value or a passphrase.
#[derive(Debug)]
pub(crate) enum VaultError {
    /// There is no vault file yet, so there is nothing to unlock.
    NoVault,
    /// The file exists but its derived key is not in memory.
    Locked,
    /// The AEAD tag did not verify: the passphrase is wrong, or the file was
    /// altered after it was written.
    WrongKey,
    /// The file could not be read or written.
    Io(std::io::Error),
    /// The file is not a vault document this build can read.
    Parse,
    /// The passphrase field was empty.
    EmptyPassphrase,
    /// An entry name or value was empty.
    InvalidEntry,
    /// The unlocked vault holds no entry under that name.
    NotFound,
    /// A named entry a provider chose is absent. Carries the id, never a value,
    /// so the refusal can say which choice is broken.
    MissingEntry(String),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoVault => write!(formatter, "no vault has been created"),
            Self::Locked => write!(formatter, "the vault is locked"),
            Self::WrongKey => write!(formatter, "the passphrase does not open the vault"),
            Self::Io(error) => {
                write!(
                    formatter,
                    "the vault file could not be read or written: {error}"
                )
            }
            Self::Parse => write!(formatter, "the vault file is not readable"),
            Self::EmptyPassphrase => write!(formatter, "the passphrase is empty"),
            Self::InvalidEntry => write!(formatter, "an entry name and value are required"),
            Self::NotFound => write!(formatter, "the vault holds no such entry"),
            Self::MissingEntry(name) => {
                write!(formatter, "the vault holds no entry named {name}")
            }
        }
    }
}

impl std::error::Error for VaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl VaultError {
    /// The locale key the settings pane shows for this refusal.
    pub(crate) fn locale_key(&self) -> &'static str {
        match self {
            Self::NoVault => "settings.vaultNoVault",
            Self::Locked => "settings.vaultLockedError",
            Self::WrongKey => "settings.vaultWrongKey",
            Self::Io(_) => "settings.vaultIo",
            Self::Parse => "settings.vaultCorrupt",
            Self::EmptyPassphrase => "settings.vaultPassphraseRequired",
            Self::InvalidEntry => "settings.vaultEntryRequired",
            Self::NotFound => "settings.vaultEntryMissing",
            Self::MissingEntry(_) => "settings.vaultNamedEntryMissing",
        }
    }

    /// The sentence to show, with a named entry's id filled in.
    ///
    /// Every other refusal is a fixed sentence; a named miss carries the id the
    /// reader chose, so the pane says which entry is missing instead of a
    /// generic absence.
    pub(crate) fn message(&self, lang: crate::i18n::Lang) -> String {
        match self {
            Self::MissingEntry(name) => {
                crate::i18n::tr_args(lang, self.locale_key(), &[("name", name.clone())])
            }
            _ => crate::i18n::tr(lang, self.locale_key()),
        }
    }
}

/// One decrypted entry: the value and the instant it was first written.
struct Secret {
    /// The confidential value. Zeroized when the vault is locked or replaced.
    value: Zeroizing<String>,
    /// Unix seconds at first write.
    created: u64,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The value is deliberately absent: a derived Debug on this type would
        // print it, and one dbg! or log line would then contain a secret.
        formatter
            .debug_struct("Secret")
            .field("value", &"<redacted>")
            .field("created", &self.created)
            .finish()
    }
}

/// The in-memory half of an unlocked vault.
struct Unlocked {
    /// The Argon2id-derived AEAD key. Dropped (and zeroized) on lock.
    key: Zeroizing<[u8; KEY_LEN]>,
    /// The salt the key was derived with, needed to re-seal on a write.
    salt: [u8; SALT_LEN],
    /// The decrypted table.
    entries: BTreeMap<String, Secret>,
}

/// One row of the unlocked listing: a name and when it was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryInfo {
    /// The entry's full namespaced name.
    pub(crate) name: String,
    /// Unix seconds at first write.
    pub(crate) created: u64,
}

/// The admin-key vault.
///
/// A Vault is a path plus, while unlocked, the derived key and the decrypted
/// table. Cloning is deliberately not implemented: the in-memory key lives in
/// exactly one place and dropping the vault drops it. The 0600 access sidecar is
/// the app's separate permission to reopen without the admin passphrase.
pub(crate) struct Vault {
    /// The file this vault owns; None for 'Vault::disabled'.
    path: Option<PathBuf>,
    /// The app-access key written beside the vault; None for 'Vault::disabled'.
    ///
    /// This is what lets a provider query its entry at launch without the admin
    /// passphrase. The file is 0600 and holds only the derived key, never the
    /// passphrase.
    access_path: Option<PathBuf>,
    /// Whether that file existed at launch or has since been created.
    ///
    /// Cached rather than stat-ed on each ask: the settings pane and every
    /// provider card read this while drawing, and a metadata syscall per row per
    /// frame is exactly the synchronous I/O the shell avoids on its input path.
    present: bool,
    /// The derived key and decrypted table, present when the app has access or
    /// the admin has unlocked.
    unlocked: Option<Unlocked>,
    /// Whether the admin passphrase has been entered for this session. The app
    /// access key alone leaves this false, which is what keeps the Vault pane
    /// locked while provider entries still resolve.
    admin_unlocked: bool,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key, never an entry.
        formatter
            .debug_struct("Vault")
            .field("path", &self.path)
            .field("app_access", &self.unlocked.is_some())
            .field("admin_unlocked", &self.admin_unlocked)
            .finish()
    }
}

/// The name a provider credential is filed under.
///
/// The provider id is the registry key the wire model carries, so the name is
/// stable across a display-name edit.
pub(crate) fn provider_key(provider: &str) -> String {
    format!("provider:{provider}:api_key")
}

impl Vault {
    /// A vault over one path, whether or not the file exists yet.
    ///
    /// An existing vault whose app-access key is present opens immediately, so
    /// providers can resolve their entries without the admin passphrase. A
    /// missing or invalid access file leaves the vault locked and the passphrase
    /// path available.
    pub(crate) fn new(path: PathBuf) -> Self {
        let present = path.is_file();
        let access_path = Some(path.with_file_name(ACCESS_FILE));
        let mut vault = Self {
            path: Some(path),
            access_path,
            present,
            unlocked: None,
            admin_unlocked: false,
        };
        if present {
            vault.open_with_access_key();
        }
        vault
    }

    /// A vault with no file behind it.
    ///
    /// Used where a build must not consult a vault at all: the tools-schema
    /// pricing path, and unit tests that exercise no credential.
    pub(crate) fn disabled() -> Self {
        Self {
            path: None,
            access_path: None,
            present: false,
            unlocked: None,
            admin_unlocked: false,
        }
    }

    /// Whether a vault file exists at this vault's path.
    pub(crate) fn exists(&self) -> bool {
        self.present
    }

    /// Whether the admin passphrase has been entered for this session.
    ///
    /// This is the Vault pane's lock: the app can still resolve provider
    /// entries from the access key while this is false.
    pub(crate) fn is_unlocked(&self) -> bool {
        self.admin_unlocked
    }

    /// Whether the app holds a decrypted table, from either its access key or
    /// the admin passphrase.
    ///
    /// Provider resolution and the provider chooser use this; the Vault pane
    /// uses [`Vault::is_unlocked`] so it still asks for the passphrase to list,
    /// reveal, or edit entries.
    pub(crate) fn is_app_available(&self) -> bool {
        self.unlocked.is_some()
    }

    /// Drop the admin view, keeping the app's access.
    ///
    /// The decrypted table stays so provider queries keep working; the Vault
    /// pane is locked again because [`Vault::is_unlocked`] is false. The access
    /// sidecar stays on disk, so a later launch also reopens for the app.
    /// Deleting that file revokes passwordless app access until the passphrase
    /// is entered again.
    pub(crate) fn lock(&mut self) {
        self.admin_unlocked = false;
    }

    /// Reopen the vault from the app-access sidecar, without the passphrase.
    ///
    /// Best-effort: a missing, unreadable, or stale key leaves the vault locked
    /// rather than surfacing an error from construction. The explicit unlock
    /// path still works and rewrites the sidecar.
    fn open_with_access_key(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let Some(access) = self.access_path.clone() else {
            return;
        };
        let bytes = match std::fs::read(&access) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(_) => return,
        };
        if bytes.len() != KEY_LEN {
            return;
        }
        let document = match read_document(&path) {
            Ok(document) => document,
            Err(_) => return,
        };
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        key.copy_from_slice(&bytes[..]);
        let plaintext = match decrypt(&key, &document) {
            Ok(plaintext) => plaintext,
            Err(_) => return,
        };
        let entries = match parse_entries(&plaintext) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        self.unlocked = Some(Unlocked {
            key,
            salt: document.salt,
            entries,
        });
    }

    /// Persist the app-access key beside the vault, owner-only.
    fn write_access_key(&self, unlocked: &Unlocked) -> Result<(), VaultError> {
        match &self.access_path {
            Some(path) => write_atomic(path, &unlocked.key[..]),
            None => Ok(()),
        }
    }

    /// Create a new empty vault, sealed with the passphrase.
    ///
    /// Refuses to overwrite an existing file: creating is for a reader who has
    /// no vault, and silently replacing one would destroy every value in it.
    pub(crate) fn create(&mut self, passphrase: &str) -> Result<(), VaultError> {
        if passphrase.is_empty() {
            return Err(VaultError::EmptyPassphrase);
        }
        let path = self.path.clone().ok_or(VaultError::NoVault)?;
        if self.present || path.exists() {
            return Err(VaultError::Io(std::io::Error::new(
                ErrorKind::AlreadyExists,
                "a vault file already exists",
            )));
        }
        let mut salt = [0u8; SALT_LEN];
        SysRng.try_fill_bytes(&mut salt).map_err(|_| {
            VaultError::Io(std::io::Error::other(
                "the operating system refused random bytes",
            ))
        })?;
        let unlocked = Unlocked {
            key: derive_key(passphrase, &salt)?,
            salt,
            entries: BTreeMap::new(),
        };
        write_document(&path, &unlocked, &unlocked.serialize(Pending::Keep))?;
        // The sidecar is the app's permission, not part of the vault's
        // integrity: a failed write leaves this session unlocked and lets the
        // next explicit unlock retry it.
        let _ = self.write_access_key(&unlocked);
        self.present = true;
        self.unlocked = Some(unlocked);
        self.admin_unlocked = true;
        Ok(())
    }

    /// Derive the key from the passphrase and decrypt the file.
    ///
    /// A failed unlock leaves both the file and any previously unlocked state
    /// untouched: the wrong passphrase changes nothing.
    pub(crate) fn unlock(&mut self, passphrase: &str) -> Result<(), VaultError> {
        if passphrase.is_empty() {
            return Err(VaultError::EmptyPassphrase);
        }
        let path = self.path.clone().ok_or(VaultError::NoVault)?;
        if !path.exists() {
            return Err(VaultError::NoVault);
        }
        // A file another process dropped in is now this vault's file, even if
        // launch found none; the cached flag is refreshed before the read.
        self.present = true;
        let document = read_document(&path)?;
        let key = derive_key(passphrase, &document.salt)?;
        let plaintext = decrypt(&key, &document)?;
        let entries = parse_entries(&plaintext)?;
        let unlocked = Unlocked {
            key,
            salt: document.salt,
            entries,
        };
        let _ = self.write_access_key(&unlocked);
        self.unlocked = Some(unlocked);
        self.admin_unlocked = true;
        Ok(())
    }

    /// Write one entry and re-seal the file.
    ///
    /// The in-memory table changes only after the new file has landed, so a
    /// write that fails leaves the unlocked state consistent with the disk.
    pub(crate) fn put(&mut self, name: &str, value: &str) -> Result<(), VaultError> {
        let name = name.trim();
        if name.is_empty() || value.is_empty() {
            return Err(VaultError::InvalidEntry);
        }
        let path = self.path.clone().ok_or(VaultError::NoVault)?;
        if !self.admin_unlocked {
            return Err(VaultError::Locked);
        }
        let unlocked = self.unlocked.as_ref().ok_or(VaultError::Locked)?;
        let created = unlocked
            .entries
            .get(name)
            .map(|entry| entry.created)
            .unwrap_or_else(crate::settings::unix_now);
        let pending = Secret {
            value: Zeroizing::new(value.to_string()),
            created,
        };
        let plaintext = unlocked.serialize(Pending::Put(name, &pending));
        write_document(&path, unlocked, &plaintext)?;
        if let Some(unlocked) = self.unlocked.as_mut() {
            unlocked.entries.insert(name.to_string(), pending);
        }
        Ok(())
    }

    /// Delete one entry and re-seal the file.
    ///
    /// Returns whether the entry existed; a missing name is not an error.
    pub(crate) fn remove(&mut self, name: &str) -> Result<bool, VaultError> {
        let path = self.path.clone().ok_or(VaultError::NoVault)?;
        if !self.admin_unlocked {
            return Err(VaultError::Locked);
        }
        let unlocked = self.unlocked.as_ref().ok_or(VaultError::Locked)?;
        if !unlocked.entries.contains_key(name) {
            return Ok(false);
        }
        let plaintext = unlocked.serialize(Pending::Remove(name));
        write_document(&path, unlocked, &plaintext)?;
        if let Some(unlocked) = self.unlocked.as_mut() {
            unlocked.entries.remove(name);
        }
        Ok(true)
    }

    /// The value under 'name', as an explicit refusal when it cannot be read.
    ///
    /// This is the admin read used by the Vault pane, so it needs the passphrase
    /// even when the app access table is present. Provider resolution uses
    /// [`Vault::resolve`] instead.
    pub(crate) fn get(&self, name: &str) -> Result<Zeroizing<String>, VaultError> {
        if !self.admin_unlocked {
            return if self.exists() {
                Err(VaultError::Locked)
            } else {
                Err(VaultError::NoVault)
            };
        }
        match &self.unlocked {
            Some(unlocked) => unlocked
                .entries
                .get(name)
                .map(|entry| entry.value.clone())
                .ok_or(VaultError::NotFound),
            None if self.exists() => Err(VaultError::Locked),
            None => Err(VaultError::NoVault),
        }
    }

    /// Whether the unlocked vault holds an entry under 'name'.
    ///
    /// A presence check, not a read: the settings pane asks this to draw a
    /// provider's credential dot without copying the secret out.
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.unlocked
            .as_ref()
            .map(|unlocked| unlocked.entries.contains_key(name))
            .unwrap_or(false)
    }

    /// The in-app query seam: the value under 'name', or none.
    ///
    /// None covers both "the vault is locked" and "the entry is absent", so a
    /// caller that must distinguish them asks 'Vault::exists' first and uses
    /// 'Vault::get' for the refusal. The provider path does exactly that; this
    /// is the cheap lookup for callers that only need the value.
    pub(crate) fn resolve(&self, name: &str) -> Option<Zeroizing<String>> {
        self.unlocked
            .as_ref()?
            .entries
            .get(name)
            .map(|entry| entry.value.clone())
    }

    /// Every entry's name and creation time, never its value.
    pub(crate) fn entries(&self) -> Result<Vec<EntryInfo>, VaultError> {
        match &self.unlocked {
            Some(unlocked) => Ok(unlocked
                .entries
                .iter()
                .map(|(name, entry)| EntryInfo {
                    name: name.clone(),
                    created: entry.created,
                })
                .collect()),
            None if self.exists() => Err(VaultError::Locked),
            None => Err(VaultError::NoVault),
        }
    }
}

/// What one seal should change relative to the table in memory.
#[derive(Clone, Copy)]
enum Pending<'a> {
    /// Write the table as it stands.
    Keep,
    /// Replace or add one entry.
    Put(&'a str, &'a Secret),
    /// Omit one entry.
    Remove(&'a str),
}

impl Unlocked {
    /// The plaintext table, with the pending change applied but not committed.
    ///
    /// Records are u32 big-endian lengths followed by the bytes, then a u64
    /// timestamp; the table starts with its own record count.
    fn serialize(&self, pending: Pending<'_>) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::new());
        out.extend_from_slice(&0u32.to_be_bytes());
        let mut count = 0u32;
        for (name, entry) in &self.entries {
            match pending {
                Pending::Remove(removed) if removed == name => continue,
                Pending::Put(replaced, replacement) if replaced == name => {
                    push_record(&mut out, name, &replacement.value, replacement.created);
                    count += 1;
                    continue;
                }
                _ => {}
            }
            push_record(&mut out, name, &entry.value, entry.created);
            count += 1;
        }
        if let Pending::Put(added, replacement) = pending {
            if !self.entries.contains_key(added) {
                push_record(&mut out, added, &replacement.value, replacement.created);
                count += 1;
            }
        }
        out[0..4].copy_from_slice(&count.to_be_bytes());
        out
    }
}

/// Append one plaintext record to the table.
fn push_record(out: &mut Vec<u8>, name: &str, value: &str, created: u64) {
    let name = name.as_bytes();
    let value = value.as_bytes();
    out.extend_from_slice(&(name.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
    out.extend_from_slice(&created.to_be_bytes());
}

/// Read one big-endian u32.
fn read_u32(table: &[u8], cursor: &mut usize) -> Result<u32, VaultError> {
    let slice = table.get(*cursor..*cursor + 4).ok_or(VaultError::Parse)?;
    *cursor += 4;
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(slice);
    Ok(u32::from_be_bytes(bytes))
}

/// Read one big-endian u64.
fn read_u64(table: &[u8], cursor: &mut usize) -> Result<u64, VaultError> {
    let slice = table.get(*cursor..*cursor + 8).ok_or(VaultError::Parse)?;
    *cursor += 8;
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(slice);
    Ok(u64::from_be_bytes(bytes))
}

/// Read one length-prefixed UTF-8 string.
fn read_string(table: &[u8], cursor: &mut usize) -> Result<String, VaultError> {
    let length = read_u32(table, cursor)? as usize;
    let start = *cursor;
    let end = start.saturating_add(length);
    let slice = table.get(start..end).ok_or(VaultError::Parse)?;
    *cursor = end;
    std::str::from_utf8(slice)
        .map(str::to_string)
        .map_err(|_| VaultError::Parse)
}

/// Decode the decrypted table.
fn parse_entries(table: &[u8]) -> Result<BTreeMap<String, Secret>, VaultError> {
    let mut cursor = 0usize;
    let count = read_u32(table, &mut cursor)?;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        let name = read_string(table, &mut cursor)?;
        let value = read_string(table, &mut cursor)?;
        let created = read_u64(table, &mut cursor)?;
        if name.is_empty() || value.is_empty() {
            return Err(VaultError::Parse);
        }
        let entry = Secret {
            value: Zeroizing::new(value),
            created,
        };
        if entries.insert(name, entry).is_some() {
            return Err(VaultError::Parse);
        }
    }
    if cursor != table.len() {
        return Err(VaultError::Parse);
    }
    Ok(entries)
}

/// The header fields a read yields, before the passphrase is known.
struct Document {
    /// The per-file salt.
    salt: [u8; SALT_LEN],
    /// The per-write nonce.
    nonce: [u8; NONCE_LEN],
    /// The sealed table.
    ciphertext: Vec<u8>,
}

/// Read and shape-check a vault file.
fn read_document(path: &Path) -> Result<Document, VaultError> {
    let bytes = std::fs::read(path).map_err(VaultError::Io)?;
    if bytes.len() < HEADER_LEN + NONCE_LEN + TAG_LEN {
        return Err(VaultError::Parse);
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(VaultError::Parse);
    }
    let version = bytes[MAGIC.len()];
    if version != VERSION {
        return Err(VaultError::Parse);
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[MAGIC.len() + 1..HEADER_LEN]);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&bytes[HEADER_LEN..HEADER_LEN + NONCE_LEN]);
    Ok(Document {
        salt,
        nonce,
        ciphertext: bytes[HEADER_LEN + NONCE_LEN..].to_vec(),
    })
}

/// The associated data: the version byte and the salt.
fn associated_data(salt: &[u8; SALT_LEN]) -> [u8; 1 + SALT_LEN] {
    let mut aad = [0u8; 1 + SALT_LEN];
    aad[0] = VERSION;
    aad[1..].copy_from_slice(salt);
    aad
}

/// Derive the AEAD key from the passphrase and the file's salt.
fn derive_key(
    passphrase: &str,
    salt: &[u8; SALT_LEN],
) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
    let params =
        Params::new(M_COST_KIB, T_COST, P_COST, Some(KEY_LEN)).map_err(|_| VaultError::Parse)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key[..])
        .map_err(|_| VaultError::Parse)?;
    Ok(key)
}

/// Decrypt the table, mapping a tag failure (including a wrong key) to
/// 'VaultError::WrongKey'.
fn decrypt(
    key: &Zeroizing<[u8; KEY_LEN]>,
    document: &Document,
) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let cipher = XChaCha20Poly1305::new_from_slice(&key[..]).map_err(|_| VaultError::Parse)?;
    let aad = associated_data(&document.salt);
    let plaintext = cipher
        .decrypt(
            &XNonce::from(document.nonce),
            Payload {
                msg: &document.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| VaultError::WrongKey)?;
    Ok(Zeroizing::new(plaintext))
}

/// Seal one plaintext table into a fresh header, nonce and ciphertext.
fn seal(unlocked: &Unlocked, plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
    let cipher =
        XChaCha20Poly1305::new_from_slice(&unlocked.key[..]).map_err(|_| VaultError::Parse)?;
    let mut nonce = [0u8; NONCE_LEN];
    SysRng.try_fill_bytes(&mut nonce).map_err(|_| {
        VaultError::Io(std::io::Error::other(
            "the operating system refused random bytes",
        ))
    })?;
    let aad = associated_data(&unlocked.salt);
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| VaultError::Io(std::io::Error::other("the vault could not be sealed")))?;
    let mut document = Vec::with_capacity(HEADER_LEN + NONCE_LEN + ciphertext.len());
    document.extend_from_slice(MAGIC);
    document.push(VERSION);
    document.extend_from_slice(&unlocked.salt);
    document.extend_from_slice(&nonce);
    document.extend_from_slice(&ciphertext);
    Ok(document)
}

/// Re-seal the table and replace the file atomically.
fn write_document(
    path: &Path,
    unlocked: &Unlocked,
    plaintext: &Zeroizing<Vec<u8>>,
) -> Result<(), VaultError> {
    let sealed = seal(unlocked, plaintext)?;
    write_atomic(path, &sealed)
}

/// Restrict a newly created file to its owner, on platforms with modes.
#[cfg(unix)]
fn private_open_options(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

/// No-op on a platform without unix modes.
#[cfg(not(unix))]
fn private_open_options(_options: &mut std::fs::OpenOptions) {}

/// Reassert owner-only permissions after the file exists.
#[cfg(unix)]
fn private_permissions(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

/// No-op on a platform without unix modes.
#[cfg(not(unix))]
fn private_permissions(_file: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

/// Write bytes to 'path' through a 0600 temporary file and a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory).map_err(VaultError::Io)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let temporary = directory.join(format!(".vault-{}-{stamp}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    private_open_options(&mut options);
    let outcome = (|| -> Result<(), VaultError> {
        let mut file = options.open(&temporary).map_err(VaultError::Io)?;
        // The umask can only remove bits, but being explicit keeps the mode
        // 0600 whatever the file was created with.
        private_permissions(&file).map_err(VaultError::Io)?;
        file.write_all(bytes).map_err(VaultError::Io)?;
        file.sync_all().map_err(VaultError::Io)?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(VaultError::Io)
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::{provider_key, EntryInfo, Vault, VaultError, MAGIC};

    /// A scratch directory this test owns, cleared first.
    fn scratch(label: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("bsh-vault-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the scratch directory");
        directory
    }

    /// A vault over a fresh scratch file.
    fn opened(label: &str) -> (std::path::PathBuf, Vault) {
        let directory = scratch(label);
        let path = directory.join(".vault.bin");
        let mut vault = Vault::new(path.clone());
        vault.create("correct horse battery").expect("the vault");
        (path, vault)
    }

    /// Create, put, lock, unlock with the same passphrase, read back.
    #[test]
    fn a_round_trip_keeps_every_entry() {
        let (path, mut vault) = opened("roundtrip");
        vault
            .put(&provider_key("work"), "sk-roundtrip-value")
            .expect("the provider key");
        vault
            .put("note:deploy", "another confidential value")
            .expect("an unnamespaced entry");
        assert_eq!(
            vault
                .resolve(&provider_key("work"))
                .map(|value| value.as_str().to_string())
                .as_deref(),
            Some("sk-roundtrip-value")
        );

        vault.lock();
        assert!(!vault.is_unlocked(), "lock drops the admin view");
        assert!(
            vault.is_app_available(),
            "the app access key keeps the table available"
        );
        assert!(
            matches!(vault.get(&provider_key("work")), Err(VaultError::Locked)),
            "the admin read refuses while locked"
        );
        assert_eq!(
            vault
                .resolve(&provider_key("work"))
                .map(|value| value.as_str().to_string())
                .as_deref(),
            Some("sk-roundtrip-value"),
            "provider resolution still works through the app access key"
        );

        vault
            .unlock("correct horse battery")
            .expect("the same passphrase");
        assert_eq!(
            vault
                .get(&provider_key("work"))
                .expect("the provider key")
                .as_str(),
            "sk-roundtrip-value"
        );
        assert_eq!(
            vault.get("note:deploy").expect("the other entry").as_str(),
            "another confidential value"
        );

        // A second open is a fresh process reading the same file.
        let mut reopened = Vault::new(path);
        reopened.unlock("correct horse battery").expect("reopen");
        assert_eq!(
            reopened
                .get(&provider_key("work"))
                .expect("still there")
                .as_str(),
            "sk-roundtrip-value"
        );
    }

    /// A wrong passphrase is WrongKey and must not touch the old file.
    #[test]
    fn a_wrong_passphrase_leaves_the_file_intact() {
        let (path, mut vault) = opened("wrong-key");
        vault
            .put(&provider_key("work"), "sk-old-value")
            .expect("the first write");
        let before = std::fs::read(&path).expect("the file");
        vault.lock();
        let refused = vault.unlock("not the passphrase");
        assert!(
            matches!(refused, Err(VaultError::WrongKey)),
            "an AEAD failure is WrongKey, got {refused:?}"
        );
        let after = std::fs::read(&path).expect("the file");
        assert_eq!(before, after, "a failed unlock must not rewrite the file");
        vault
            .unlock("correct horse battery")
            .expect("the right passphrase still opens it");
        assert_eq!(
            vault
                .get(&provider_key("work"))
                .expect("the value")
                .as_str(),
            "sk-old-value"
        );
    }

    /// Neither the secret nor the passphrase appears anywhere in the bytes.
    #[test]
    fn the_file_contains_neither_the_secret_nor_the_passphrase() {
        let directory = scratch("bytes");
        let path = directory.join(".vault.bin");
        let passphrase = "passphrase-not-in-the-file-123";
        let secret = "sk-live-this-must-not-be-findable-456";
        let mut vault = Vault::new(path.clone());
        vault.create(passphrase).expect("the vault");
        vault
            .put(&provider_key("openai"), secret)
            .expect("the entry");
        let bytes = std::fs::read(&path).expect("the sealed file");
        let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|window| window == needle);
        assert!(!contains(secret.as_bytes()), "the secret is in the file");
        assert!(
            !contains(passphrase.as_bytes()),
            "the passphrase is in the file"
        );
        assert!(contains(MAGIC), "the file carries the vault magic");
    }

    /// The explicit errors a caller can act on.
    #[test]
    fn missing_and_malformed_files_are_named() {
        let directory = scratch("errors");
        let path = directory.join(".vault.bin");
        let mut absent = Vault::new(path.clone());
        assert!(matches!(absent.get("anything"), Err(VaultError::NoVault)));
        assert!(matches!(absent.entries(), Err(VaultError::NoVault)));
        assert!(!absent.exists());

        std::fs::write(&path, b"not a vault").expect("garbage");
        let refused = absent.unlock("whatever");
        assert!(
            matches!(refused, Err(VaultError::Parse)),
            "garbage is Parse, got {refused:?}"
        );
        assert!(matches!(absent.get("anything"), Err(VaultError::Locked)));
        assert!(matches!(absent.entries(), Err(VaultError::Locked)));
    }

    /// The listing carries names and creation times, never values, and a delete
    /// is persisted.
    #[test]
    fn the_listing_hides_values_and_delete_persists() {
        let (path, mut vault) = opened("listing");
        vault.put("alpha", "sk-one-secret").expect("alpha");
        vault.put("beta", "sk-two-secret").expect("beta");
        let listed: Vec<EntryInfo> = vault.entries().expect("the listing");
        assert_eq!(
            listed
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"],
            "the listing is name-ordered"
        );
        assert!(listed.iter().all(|entry| entry.created > 0), "and dated");
        let described = format!("{listed:?}");
        assert!(
            !described.contains("sk-one-secret") && !described.contains("sk-two-secret"),
            "a listing debug string never carries a value"
        );

        assert!(
            vault.remove("alpha").expect("the delete"),
            "alpha was there"
        );
        assert!(!vault.remove("alpha").expect("the second delete"));
        assert!(matches!(vault.get("alpha"), Err(VaultError::NotFound)));
        let mut reopened = Vault::new(path);
        reopened.unlock("correct horse battery").expect("reopen");
        assert!(matches!(reopened.get("alpha"), Err(VaultError::NotFound)));
        assert_eq!(
            reopened.get("beta").expect("beta").as_str(),
            "sk-two-secret"
        );
    }

    /// An empty passphrase is refused rather than creating an unprotected file.
    #[test]
    fn an_empty_passphrase_never_creates_a_vault() {
        let directory = scratch("empty");
        let path = directory.join(".vault.bin");
        let mut vault = Vault::new(path.clone());
        assert!(matches!(vault.create(""), Err(VaultError::EmptyPassphrase)));
        assert!(!path.exists(), "nothing was written");
    }

    /// Creating twice refuses instead of destroying the values already there.
    #[test]
    fn an_existing_vault_is_never_overwritten() {
        let (path, mut vault) = opened("exists");
        vault.put("keep", "me").expect("the entry");
        let before = std::fs::read(&path).expect("the file");
        let refused = vault.create("a second passphrase");
        assert!(
            matches!(refused, Err(VaultError::Io(ref error)) if error.kind() == std::io::ErrorKind::AlreadyExists),
            "an existing vault is refused, got {refused:?}"
        );
        assert_eq!(std::fs::read(&path).expect("the file"), before);
    }

    /// The app-access key reopens the vault at launch without the passphrase.
    #[test]
    fn the_app_reopens_with_its_access_key_not_the_passphrase() {
        let (path, mut vault) = opened("app-access");
        vault
            .put(&provider_key("work"), "sk-app-access")
            .expect("the provider entry");
        drop(vault);

        let mut reopened = Vault::new(path);
        assert!(
            !reopened.is_unlocked(),
            "the admin view stays locked at launch"
        );
        assert!(
            reopened.is_app_available(),
            "the app access key opens the table for provider resolution"
        );
        assert_eq!(
            reopened
                .resolve(&provider_key("work"))
                .map(|value| value.as_str().to_string())
                .as_deref(),
            Some("sk-app-access")
        );
        assert!(
            matches!(reopened.get(&provider_key("work")), Err(VaultError::Locked)),
            "listing/revealing still needs the admin passphrase"
        );
        assert_eq!(
            reopened
                .entries()
                .expect("entry names are available to the chooser")
                .len(),
            1
        );
        reopened
            .unlock("correct horse battery")
            .expect("the admin passphrase");
        assert_eq!(
            reopened
                .get(&provider_key("work"))
                .expect("the provider entry")
                .as_str(),
            "sk-app-access"
        );
    }

    /// Locking the admin view leaves provider resolution working, which is the
    /// split the app-access key exists for.
    #[test]
    fn locking_the_admin_view_keeps_app_access() {
        let (_path, mut vault) = opened("lock-keeps-app");
        vault
            .put(&provider_key("work"), "sk-still-available")
            .expect("the provider entry");
        vault.lock();
        assert!(!vault.is_unlocked(), "the Vault pane is locked");
        assert!(vault.is_app_available(), "the app still holds the table");
        assert_eq!(
            vault
                .resolve(&provider_key("work"))
                .map(|value| value.as_str().to_string())
                .as_deref(),
            Some("sk-still-available")
        );
        assert!(matches!(
            vault.get(&provider_key("work")),
            Err(VaultError::Locked)
        ));
    }

    /// Deleting the app-access key revokes passwordless access until the admin
    /// passphrase is entered, which writes a fresh key.
    #[test]
    fn removing_the_access_key_relocks_the_app() {
        let (path, mut vault) = opened("revoke-access");
        vault
            .put(&provider_key("work"), "sk-revoked-access")
            .expect("the provider entry");
        let access = path.with_file_name(".vault.access");
        std::fs::remove_file(&access).expect("the access file");
        drop(vault);

        let mut reopened = Vault::new(path);
        assert!(!reopened.is_unlocked(), "no access key means admin locked");
        assert!(
            !reopened.is_app_available(),
            "and the app has no decrypted table"
        );
        assert!(
            reopened.resolve(&provider_key("work")).is_none(),
            "provider resolution cannot read it either"
        );
        assert!(matches!(
            reopened.get(&provider_key("work")),
            Err(VaultError::Locked)
        ));
        reopened
            .unlock("correct horse battery")
            .expect("the admin passphrase");
        assert_eq!(
            reopened
                .get(&provider_key("work"))
                .expect("the provider entry")
                .as_str(),
            "sk-revoked-access"
        );
        assert!(
            access.is_file(),
            "an explicit unlock writes the access key again"
        );
    }

    /// The on-disk mode is 0600 where the platform has modes.
    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (path, _vault) = opened("mode");
        for file in [path.clone(), path.with_file_name(".vault.access")] {
            let mode = std::fs::metadata(&file)
                .expect("the file")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{} is owner-only", file.display());
        }
    }
}
