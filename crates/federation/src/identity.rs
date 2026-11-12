//! Node identity: an Ed25519 key that a NodeId names.
//!
//! A NodeId is the base64 public key. It is deliberately not a hostname, a
//! Zenoh id, or a display name: those are endpoints and labels, and a Node
//! keeps its identity when either changes (RFC section 11.2).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use capability::NodeId;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// Why a key could not be read.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// The value is not valid base64.
    #[error("the key is not valid base64: {0}")]
    Base64(String),
    /// The decoded value is the wrong length.
    #[error("the key is not {expected} bytes")]
    Length {
        /// The required length.
        expected: usize,
    },
    /// The decoded value is not a valid Ed25519 point.
    #[error("the key is not a valid Ed25519 key")]
    Key,
    /// The value cannot form a Node id.
    #[error("{0}")]
    Node(String),
    /// The seed file could not be read or written.
    #[error("cannot access {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The operating system's error.
        source: std::io::Error,
    },
    /// The operating system's random source failed.
    #[error("no random source is available: {0}")]
    Random(String),
}

/// The signing half of a Node identity.
#[derive(Clone)]
pub struct NodeIdentity {
    signing: SigningKey,
}

impl NodeIdentity {
    /// Generate a fresh identity from the operating system's entropy.
    pub fn generate() -> Result<Self, IdentityError> {
        Ok(Self::from_seed(random_seed()?))
    }

    /// Write this identity's seed as base64, private to the account.
    ///
    /// The file is the identity; a later load_or_create reads exactly this.
    /// The write is atomic and owner-only on every host (issue #299): Unix
    /// mode 0600, Windows a protected owner-only DACL, and a pre-existing
    /// world-readable file is hardened by the replacement. A failure to
    /// establish the private policy fails the save.
    pub fn save(&self, path: &std::path::Path) -> Result<(), IdentityError> {
        hostkit::fspriv::write_private(path, STANDARD.encode(self.signing.to_bytes()).as_bytes())
            .map_err(|source| IdentityError::Io {
                path: path.display().to_string(),
                source,
            })
    }

    /// Build an identity from a 32-byte seed.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    /// Store this identity's seed through a custody backend.
    pub fn save_custody(
        &self,
        custody: &dyn crate::custody::Custody,
        name: &str,
    ) -> Result<(), crate::custody::CustodyError> {
        custody.store(name, STANDARD.encode(self.signing.to_bytes()).as_bytes())
    }

    /// Load an identity from a custody backend, when the name is stored.
    pub fn load_custody(
        custody: &dyn crate::custody::Custody,
        name: &str,
    ) -> Result<Option<Self>, crate::custody::CustodyError> {
        let Some(secret) = custody.load(name)? else {
            return Ok(None);
        };
        let text =
            std::str::from_utf8(&secret).map_err(|_| crate::custody::CustodyError::Format)?;
        Self::from_seed_base64(text)
            .map(Some)
            .map_err(|_| crate::custody::CustodyError::Format)
    }

    /// Build an identity from a base64 32-byte seed.
    pub fn from_seed_base64(value: &str) -> Result<Self, IdentityError> {
        let bytes = STANDARD
            .decode(value.trim())
            .map_err(|error| IdentityError::Base64(error.to_string()))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| IdentityError::Length { expected: 32 })?;
        Ok(Self::from_seed(seed))
    }

    /// The Node id this identity answers to.
    pub fn node_id(&self) -> NodeId {
        NodeId::new(self.public_key()).expect("a base64 key is never an empty id")
    }

    /// The base64 public key.
    pub fn public_key(&self) -> String {
        STANDARD.encode(self.signing.verifying_key().to_bytes())
    }

    /// Sign bytes, returning a base64 signature.
    pub fn sign(&self, message: &[u8]) -> String {
        STANDARD.encode(self.signing.sign(message).to_bytes())
    }

    /// Load the Node's seed file, creating it when it does not exist.
    ///
    /// The file is the Node's identity: a base64 32-byte seed. It is created
    /// owner-only on every host (issue #299): Unix mode 0600, Windows a
    /// protected owner-only DACL. Losing the file loses the identity, so a
    /// caller that needs to keep its grants backs it up like a key.
    pub fn load_or_create(path: &std::path::Path) -> Result<Self, IdentityError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_seed_base64(text.trim()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let seed = random_seed()?;
                hostkit::fspriv::write_private(path, STANDARD.encode(seed).as_bytes()).map_err(
                    |source| IdentityError::Io {
                        path: path.display().to_string(),
                        source,
                    },
                )?;
                Ok(Self::from_seed(seed))
            }
            Err(source) => Err(IdentityError::Io {
                path: path.display().to_string(),
                source,
            }),
        }
    }
}

fn random_seed() -> Result<[u8; 32], IdentityError> {
    use rand::rngs::SysRng;
    use rand::TryRng as _;
    let mut seed = [0u8; 32];
    SysRng
        .try_fill_bytes(&mut seed)
        .map_err(|error| IdentityError::Random(error.to_string()))?;
    Ok(seed)
}

/// Verify a base64 signature against the public key a Node id names.
pub fn verify(node: &NodeId, message: &[u8], signature: &str) -> bool {
    let Ok(key) = decode_key(node.as_str()) else {
        return false;
    };
    let Ok(bytes) = STANDARD.decode(signature) else {
        return false;
    };
    let Ok(bytes) = <[u8; 64]>::try_from(bytes) else {
        return false;
    };
    let signature = Signature::from_bytes(&bytes);
    key.verify_strict(message, &signature).is_ok()
}

/// Whether a Node id names a well-formed Ed25519 public key.
pub fn is_valid_node_id(node: &NodeId) -> bool {
    decode_key(node.as_str()).is_ok()
}

fn decode_key(value: &str) -> Result<VerifyingKey, IdentityError> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|error| IdentityError::Base64(error.to_string()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| IdentityError::Length { expected: 32 })?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| IdentityError::Key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "federation-identity-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// Issue #299: the seed file is owner-only on creation, and saving over a
    /// pre-existing world-readable file hardens it without losing the identity.
    #[test]
    fn the_seed_file_is_private_and_hardens_a_replaced_file() {
        let dir = scratch("private");
        let path = dir.join("node.key");

        let created = NodeIdentity::load_or_create(&path).expect("create");
        let reloaded = NodeIdentity::load_or_create(&path).expect("reload");
        assert_eq!(created.node_id(), reloaded.node_id());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "a fresh seed file must be owner-only");
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }
        let replacement = NodeIdentity::from_seed([9u8; 32]);
        replacement.save(&path).expect("save");
        assert_eq!(
            NodeIdentity::load_or_create(&path).expect("load").node_id(),
            replacement.node_id()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "the replaced seed file must be owner-only"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
