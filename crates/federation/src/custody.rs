//! Where a node's private key lives.
//!
//! A custody backend holds one opaque secret per name. The default file backend
//! keeps a 0600 file in a directory the host owns; the passphrase backend wraps
//! the secret in an Argon2id-derived XChaCha20-Poly1305 document, so the key is
//! not readable from the filesystem alone. Platform keychains are a separate
//! backend (tracked as follow-up work); the identity only needs this trait.
//!
//! Backup and recovery use the same secret the backend stores: an encrypted
//! passphrase file can be copied to another machine and opened with the same
//! passphrase; a file-backend secret is the plaintext key and must be handled
//! accordingly.

use std::io::Write;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::rngs::SysRng;
use rand::TryRng;
use zeroize::Zeroizing;

/// The encrypted-custody file magic.
const MAGIC: &[u8; 4] = b"BSHC";
/// The only format version this build writes or reads.
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const M_COST_KIB: u32 = 65_536;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

/// Why custody failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CustodyError {
    /// The backend could not be read or written.
    #[error("cannot access {path}: {message}")]
    Io {
        /// Where.
        path: String,
        /// What happened.
        message: String,
    },
    /// The document is not a custody document.
    #[error("the custody document is malformed")]
    Format,
    /// The passphrase did not open the document.
    #[error("the passphrase did not open the custody document")]
    WrongPassphrase,
}

/// One place a node's secret can be kept.
pub trait Custody: Send + Sync {
    /// A short, secret-free description for diagnostics.
    fn describe(&self) -> String;
    /// Read the secret; `None` when the name is not stored.
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, CustodyError>;
    /// Store or replace the secret.
    fn store(&self, name: &str, secret: &[u8]) -> Result<(), CustodyError>;
    /// Remove the secret; a missing name is not an error.
    fn delete(&self, name: &str) -> Result<(), CustodyError>;
}

/// Only letters, digits, '-' and '_' survive a name; everything else is '-'.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "node".to_string()
    } else {
        cleaned
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CustodyError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| CustodyError::Io {
            path: parent.display().to_string(),
            message: error.to_string(),
        })?;
    }
    let temporary = path.with_extension("part");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|error| CustodyError::Io {
            path: temporary.display().to_string(),
            message: error.to_string(),
        })?;
        file.write_all(bytes).map_err(|error| CustodyError::Io {
            path: temporary.display().to_string(),
            message: error.to_string(),
        })?;
        file.flush().map_err(|error| CustodyError::Io {
            path: temporary.display().to_string(),
            message: error.to_string(),
        })?;
    }
    std::fs::rename(&temporary, path).map_err(|error| CustodyError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

/// One 0600 file per name inside a directory.
#[derive(Debug, Clone)]
pub struct FileCustody {
    directory: PathBuf,
}

impl FileCustody {
    /// Keep secrets in `directory`.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(format!("{}.key", safe_name(name)))
    }
}

impl Custody for FileCustody {
    fn describe(&self) -> String {
        format!("file:{}", self.directory.display())
    }

    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, CustodyError> {
        let path = self.path(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(CustodyError::Io {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }

    fn store(&self, name: &str, secret: &[u8]) -> Result<(), CustodyError> {
        write_private(&self.path(name), secret)
    }

    fn delete(&self, name: &str) -> Result<(), CustodyError> {
        let path = self.path(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CustodyError::Io {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }
}

/// A passphrase-wrapped store: Argon2id + XChaCha20-Poly1305 per file.
#[derive(Debug, Clone)]
pub struct PassphraseCustody {
    directory: PathBuf,
    passphrase: Zeroizing<String>,
}

impl PassphraseCustody {
    /// Keep encrypted secrets in `directory`, opened by `passphrase`.
    pub fn new(directory: impl Into<PathBuf>, passphrase: impl Into<String>) -> Self {
        Self {
            directory: directory.into(),
            passphrase: Zeroizing::new(passphrase.into()),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(format!("{}.custody", safe_name(name)))
    }

    fn key(&self, salt: &[u8; SALT_LEN]) -> Result<Zeroizing<[u8; KEY_LEN]>, CustodyError> {
        let params = Params::new(M_COST_KIB, T_COST, P_COST, Some(KEY_LEN))
            .map_err(|_| CustodyError::Format)?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        argon
            .hash_password_into(self.passphrase.as_bytes(), salt, &mut key[..])
            .map_err(|_| CustodyError::Format)?;
        Ok(key)
    }
}

impl Custody for PassphraseCustody {
    fn describe(&self) -> String {
        format!("passphrase:{}", self.directory.display())
    }

    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, CustodyError> {
        let path = self.path(name);
        let document = match std::fs::read(&path) {
            Ok(document) => document,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(CustodyError::Io {
                    path: path.display().to_string(),
                    message: error.to_string(),
                })
            }
        };
        if document.len() < 5 + SALT_LEN + NONCE_LEN || &document[0..4] != MAGIC {
            return Err(CustodyError::Format);
        }
        if document[4] != VERSION {
            return Err(CustodyError::Format);
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&document[5..5 + SALT_LEN]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&document[5 + SALT_LEN..5 + SALT_LEN + NONCE_LEN]);
        let ciphertext = &document[5 + SALT_LEN + NONCE_LEN..];
        let key = self.key(&salt)?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(&key[..]).map_err(|_| CustodyError::Format)?;
        let mut aad = Vec::with_capacity(1 + SALT_LEN);
        aad.push(VERSION);
        aad.extend_from_slice(&salt);
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map(Some)
            .map_err(|_| CustodyError::WrongPassphrase)
    }

    fn store(&self, name: &str, secret: &[u8]) -> Result<(), CustodyError> {
        let mut salt = [0u8; SALT_LEN];
        SysRng
            .try_fill_bytes(&mut salt)
            .map_err(|_| CustodyError::Format)?;
        let key = self.key(&salt)?;
        let mut nonce = [0u8; NONCE_LEN];
        SysRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| CustodyError::Format)?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(&key[..]).map_err(|_| CustodyError::Format)?;
        let mut aad = Vec::with_capacity(1 + SALT_LEN);
        aad.push(VERSION);
        aad.extend_from_slice(&salt);
        let ciphertext = cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: secret,
                    aad: &aad,
                },
            )
            .map_err(|_| CustodyError::Format)?;
        let mut document = Vec::with_capacity(5 + SALT_LEN + NONCE_LEN + ciphertext.len());
        document.extend_from_slice(MAGIC);
        document.push(VERSION);
        document.extend_from_slice(&salt);
        document.extend_from_slice(&nonce);
        document.extend_from_slice(&ciphertext);
        write_private(&self.path(name), &document)
    }

    fn delete(&self, name: &str) -> Result<(), CustodyError> {
        let path = self.path(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CustodyError::Io {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }
}

/// Move a secret between backends.
///
/// The target stores first and the source is deleted only after that succeeds,
/// so a failed migration leaves the original in place. Returns whether a secret
/// was found.
pub fn migrate(from: &dyn Custody, to: &dyn Custody, name: &str) -> Result<bool, CustodyError> {
    let Some(secret) = from.load(name)? else {
        return Ok(false);
    };
    to.store(name, &secret)?;
    from.delete(name)?;
    Ok(true)
}

/// A platform keychain backend.
///
/// macOS uses `security` generic passwords; Linux uses `secret-tool` (Secret
/// Service). Windows and any host without the tool report `available() == false`
/// and the caller falls back explicitly.
///
/// macOS note: `security add-generic-password -w` takes the secret on the
/// command line, so it is briefly visible in the process list. The keychain
/// itself encrypts it at rest; a prompt-based flow is not available to a
/// headless launch.
#[derive(Debug, Clone)]
pub struct KeychainCustody {
    service: String,
}

impl KeychainCustody {
    /// Keychain entries live under one service name.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// Whether this host has a usable keychain tool.
    pub fn available() -> bool {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("security")
                .arg("-h")
                .output()
                .map(|output| output.status.success() || !output.stderr.is_empty())
                .unwrap_or(false)
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("secret-tool")
                .arg("--help")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            false
        }
    }

    /// The service name.
    pub fn service(&self) -> &str {
        &self.service
    }
}

#[cfg(target_os = "macos")]
fn run(command: &mut std::process::Command) -> Result<Option<Vec<u8>>, CustodyError> {
    let output = command.output().map_err(|error| CustodyError::Io {
        path: "keychain".to_string(),
        message: error.to_string(),
    })?;
    if output.status.success() {
        return Ok(Some(output.stdout));
    }
    let message = String::from_utf8_lossy(&output.stderr).to_string();
    if message.contains("could not be found") {
        return Ok(None);
    }
    Err(CustodyError::Io {
        path: "keychain".to_string(),
        message,
    })
}

#[cfg(target_os = "macos")]
impl Custody for KeychainCustody {
    fn describe(&self) -> String {
        format!("keychain:{}", self.service)
    }

    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, CustodyError> {
        let output = run(std::process::Command::new("security")
            .arg("find-generic-password")
            .arg("-s")
            .arg(&self.service)
            .arg("-a")
            .arg(name)
            .arg("-w"))?;
        Ok(output.map(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            text.trim_end_matches(['\n', '\r']).as_bytes().to_vec()
        }))
    }

    fn store(&self, name: &str, secret: &[u8]) -> Result<(), CustodyError> {
        let secret = String::from_utf8_lossy(secret).to_string();
        let output = std::process::Command::new("security")
            .arg("add-generic-password")
            .arg("-U")
            .arg("-s")
            .arg(&self.service)
            .arg("-a")
            .arg(name)
            .arg("-w")
            .arg(&secret)
            .output()
            .map_err(|error| CustodyError::Io {
                path: "keychain".to_string(),
                message: error.to_string(),
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(CustodyError::Io {
                path: "keychain".to_string(),
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            })
        }
    }

    fn delete(&self, name: &str) -> Result<(), CustodyError> {
        let output = std::process::Command::new("security")
            .arg("delete-generic-password")
            .arg("-s")
            .arg(&self.service)
            .arg("-a")
            .arg(name)
            .output()
            .map_err(|error| CustodyError::Io {
                path: "keychain".to_string(),
                message: error.to_string(),
            })?;
        if output.status.success()
            || String::from_utf8_lossy(&output.stderr).contains("could not be found")
        {
            Ok(())
        } else {
            Err(CustodyError::Io {
                path: "keychain".to_string(),
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            })
        }
    }
}

#[cfg(target_os = "linux")]
impl Custody for KeychainCustody {
    fn describe(&self) -> String {
        format!("keychain:{}", self.service)
    }

    fn load(&self, name: &str) -> Result<Option<Vec<u8>>, CustodyError> {
        let output = std::process::Command::new("secret-tool")
            .arg("lookup")
            .arg("service")
            .arg(&self.service)
            .arg("account")
            .arg(name)
            .output()
            .map_err(|error| CustodyError::Io {
                path: "keychain".to_string(),
                message: error.to_string(),
            })?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(output.stdout))
    }

    fn store(&self, name: &str, secret: &[u8]) -> Result<(), CustodyError> {
        use std::io::Write as _;
        let mut child = std::process::Command::new("secret-tool")
            .arg("store")
            .arg(format!("--label=bsh-federation {name}"))
            .arg("service")
            .arg(&self.service)
            .arg("account")
            .arg(name)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| CustodyError::Io {
                path: "keychain".to_string(),
                message: error.to_string(),
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(secret).map_err(|error| CustodyError::Io {
                path: "keychain".to_string(),
                message: error.to_string(),
            })?;
        }
        let status = child.wait().map_err(|error| CustodyError::Io {
            path: "keychain".to_string(),
            message: error.to_string(),
        })?;
        if status.success() {
            Ok(())
        } else {
            Err(CustodyError::Io {
                path: "keychain".to_string(),
                message: "secret-tool refused the entry".to_string(),
            })
        }
    }

    fn delete(&self, name: &str) -> Result<(), CustodyError> {
        let output = std::process::Command::new("secret-tool")
            .arg("clear")
            .arg("service")
            .arg(&self.service)
            .arg("account")
            .arg(name)
            .output()
            .map_err(|error| CustodyError::Io {
                path: "keychain".to_string(),
                message: error.to_string(),
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(CustodyError::Io {
                path: "keychain".to_string(),
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            })
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl Custody for KeychainCustody {
    fn describe(&self) -> String {
        format!("keychain:{} (unsupported)", self.service)
    }

    fn load(&self, _name: &str) -> Result<Option<Vec<u8>>, CustodyError> {
        Ok(None)
    }

    fn store(&self, _name: &str, _secret: &[u8]) -> Result<(), CustodyError> {
        Err(CustodyError::Io {
            path: "keychain".to_string(),
            message: "this platform has no keychain backend".to_string(),
        })
    }

    fn delete(&self, _name: &str) -> Result<(), CustodyError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("federation-custody-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn a_file_secret_is_0600_and_round_trips() {
        let dir = scratch("file");
        let custody = FileCustody::new(&dir);
        custody.store("node-a", b"secret-bytes").expect("store");
        assert_eq!(
            custody.load("node-a").expect("load").expect("present"),
            b"secret-bytes"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("node-a.key"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        custody.delete("node-a").expect("delete");
        assert!(custody.load("node-a").expect("load").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_passphrase_wraps_and_rejects_the_wrong_one() {
        let dir = scratch("passphrase");
        let custody = PassphraseCustody::new(&dir, "correct horse");
        custody.store("node-a", b"secret-bytes").expect("store");
        assert_eq!(
            custody.load("node-a").expect("load").expect("present"),
            b"secret-bytes"
        );

        let wrong = PassphraseCustody::new(&dir, "wrong horse");
        assert_eq!(wrong.load("node-a"), Err(CustodyError::WrongPassphrase));

        let path = dir.join("node-a.custody");
        let mut document = std::fs::read(&path).expect("read");
        let last = document.len() - 1;
        document[last] ^= 0x01;
        std::fs::write(&path, document).expect("write");
        assert!(custody.load("node-a").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_cannot_escape_the_directory() {
        let custody = FileCustody::new("/tmp/custody-names");
        let name = "../../etc/passwd";
        let path = custody.path(name);
        assert!(path.starts_with("/tmp/custody-names"));
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn an_identity_round_trips_through_custody() {
        use crate::custody::{FileCustody, PassphraseCustody};
        let dir = scratch("identity");
        let identity = crate::identity::NodeIdentity::from_seed([7u8; 32]);
        let custody = FileCustody::new(dir.join("file"));
        identity.save_custody(&custody, "node-a").expect("save");
        let loaded = crate::identity::NodeIdentity::load_custody(&custody, "node-a")
            .expect("load")
            .expect("present");
        assert_eq!(loaded.node_id(), identity.node_id());

        let encrypted = PassphraseCustody::new(dir.join("wrapped"), "backup phrase");
        identity.save_custody(&encrypted, "node-a").expect("save");
        let loaded = crate::identity::NodeIdentity::load_custody(&encrypted, "node-a")
            .expect("load")
            .expect("present");
        assert_eq!(loaded.node_id(), identity.node_id());

        let wrong = PassphraseCustody::new(dir.join("wrapped"), "wrong");
        assert!(crate::identity::NodeIdentity::load_custody(&wrong, "node-a").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_migrates_without_losing_the_original_on_failure() {
        let dir = scratch("migrate");
        let source = FileCustody::new(dir.join("source"));
        let target = PassphraseCustody::new(dir.join("target"), "backup phrase");
        source.store("node-a", b"secret-bytes").expect("store");
        assert!(migrate(&source, &target, "node-a").expect("migrated"));
        assert!(source.load("node-a").expect("load").is_none());
        assert_eq!(
            target.load("node-a").expect("load").expect("present"),
            b"secret-bytes"
        );
        // Nothing to move: a no-op, not an error.
        assert!(!migrate(&source, &target, "node-a").expect("no-op"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
