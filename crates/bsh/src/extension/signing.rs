//! Publisher identity and package signatures.
//!
//! A package signature is Ed25519 over a canonical message naming the
//! extension id, exact version and package hash. Verification needs a trusted
//! public key from extension-state/trust.toml; a signature alone is not a grant
//! of trust. A signature proves origin and integrity, never benign behaviour.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use super::capability::unix_now;
use super::manifest::Manifest;
use super::package::{package_hash, write_atomic};
use super::types::{ExtensionError, TrustLevel};

/// The signature file inside a package.
pub(crate) const SIGNATURE_FILE: &str = "package.sig";

/// The signature schema generation.
pub(crate) const SIGNATURE_SCHEMA: &str = "1.0";

/// The signature algorithm this build accepts.
pub(crate) const SIGNATURE_ALGORITHM: &str = "ed25519";

/// The signature metadata shipped beside a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackageSignature {
    /// Signature schema generation.
    pub(crate) schema_version: String,
    /// Publisher name the trust store keys on.
    pub(crate) publisher: String,
    /// Base64 Ed25519 public key.
    pub(crate) public_key: String,
    /// Signature algorithm.
    pub(crate) algorithm: String,
    /// Base64 Ed25519 signature.
    pub(crate) signature: String,
}

impl PackageSignature {
    /// Read package.sig from a package root, if present.
    pub(crate) fn load(root: &Path) -> Result<Option<Self>, ExtensionError> {
        let path = root.join(SIGNATURE_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        let signature: Self =
            serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
                path,
                message: error.to_string(),
            })?;
        Ok(Some(signature))
    }
}

/// The canonical message a signature covers.
pub(crate) fn signed_message(id: &str, version: &str, hash: &str) -> Vec<u8> {
    format!("bsh-package-v1\n{id}\n{version}\n{hash}\n").into_bytes()
}

/// Sign a package in place, writing package.sig.
///
/// The secret key is a base64 32-byte Ed25519 seed. The public key is derived
/// and recorded in the signature file; trust in that publisher is a separate
/// step, performed by the trust CLI command.
pub(crate) fn sign_package(
    root: &Path,
    publisher: &str,
    secret_key_base64: &str,
) -> Result<PackageSignature, ExtensionError> {
    let manifest = Manifest::load(root)?;
    let hash = package_hash(root)?;
    let secret = STANDARD
        .decode(secret_key_base64.trim())
        .map_err(|error| ExtensionError::InvalidVersion {
            value: "secret key".to_string(),
            reason: error.to_string(),
        })?;
    let secret: [u8; 32] = secret.try_into().map_err(|_| {
        ExtensionError::InvalidVersion {
            value: "secret key".to_string(),
            reason: "an Ed25519 seed is exactly 32 bytes".to_string(),
        }
    })?;
    let signing = SigningKey::from_bytes(&secret);
    let message = signed_message(
        manifest.id().as_str(),
        &manifest.version().to_string(),
        &hash,
    );
    let signature = signing.sign(&message);
    let file = PackageSignature {
        schema_version: SIGNATURE_SCHEMA.to_string(),
        publisher: publisher.to_string(),
        public_key: STANDARD.encode(signing.verifying_key().to_bytes()),
        algorithm: SIGNATURE_ALGORITHM.to_string(),
        signature: STANDARD.encode(signature.to_bytes()),
    };
    let text = serde_json::to_string_pretty(&file).map_err(|error| ExtensionError::Json {
        path: root.join(SIGNATURE_FILE),
        message: error.to_string(),
    })?;
    write_atomic(&root.join(SIGNATURE_FILE), text.as_bytes())?;
    Ok(file)
}

/// Generate a new key pair. Returns (secret base64, public base64).
pub(crate) fn generate_key() -> (String, String) {
    use rand::rngs::SysRng;
    use rand::TryRng as _;
    let mut seed = [0u8; 32];
    SysRng
        .try_fill_bytes(&mut seed)
        .expect("the operating system random source is available");
    let signing = SigningKey::from_bytes(&seed);
    (
        STANDARD.encode(seed),
        STANDARD.encode(signing.verifying_key().to_bytes()),
    )
}

/// One trusted publisher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PublisherTrust {
    /// Base64 Ed25519 public key.
    pub(crate) public_key: String,
    /// Unix seconds the key was trusted.
    #[serde(default)]
    pub(crate) trusted_at: u64,
    /// Why the key was trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

/// The persisted publisher trust document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TrustFile {
    /// Format generation.
    #[serde(default = "trust_format")]
    pub(crate) format: u32,
    /// publisher name -> key.
    #[serde(default)]
    pub(crate) publishers: BTreeMap<String, PublisherTrust>,
}

fn trust_format() -> u32 {
    1
}

impl Default for TrustFile {
    fn default() -> Self {
        Self {
            format: trust_format(),
            publishers: BTreeMap::new(),
        }
    }
}

/// The trust store.
pub(crate) struct TrustStore {
    path: PathBuf,
    file: TrustFile,
}

impl TrustStore {
    /// Load or create the trust store.
    pub(crate) fn load(path: PathBuf) -> Result<Self, ExtensionError> {
        let file = if path.is_file() {
            let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
                path: path.clone(),
                source,
            })?;
            toml::from_str(&text).map_err(|error| ExtensionError::Corrupt {
                path: path.clone(),
                message: error.to_string(),
            })?
        } else {
            TrustFile::default()
        };
        Ok(Self { path, file })
    }

    /// The file path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Trust a publisher key.
    pub(crate) fn trust(
        &mut self,
        publisher: &str,
        public_key: &str,
        note: Option<String>,
    ) -> Result<(), ExtensionError> {
        self.file.publishers.insert(
            publisher.to_string(),
            PublisherTrust {
                public_key: public_key.to_string(),
                trusted_at: unix_now(),
                note,
            },
        );
        self.save()
    }

    /// Remove a publisher.
    pub(crate) fn revoke(&mut self, publisher: &str) -> Result<bool, ExtensionError> {
        let removed = self.file.publishers.remove(publisher).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// The trusted key for a publisher.
    pub(crate) fn key(&self, publisher: &str) -> Option<&str> {
        self.file
            .publishers
            .get(publisher)
            .map(|trust| trust.public_key.as_str())
    }

    /// All trusted publishers.
    pub(crate) fn publishers(&self) -> Vec<(String, PublisherTrust)> {
        self.file
            .publishers
            .iter()
            .map(|(name, trust)| (name.clone(), trust.clone()))
            .collect()
    }

    fn save(&self) -> Result<(), ExtensionError> {
        let text =
            toml::to_string_pretty(&self.file).map_err(|error| ExtensionError::Corrupt {
                path: self.path.clone(),
                message: error.to_string(),
            })?;
        write_atomic(&self.path, text.as_bytes())
    }
}

/// The result of checking a package's signature against the trust store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum SignatureVerification {
    /// The package ships no signature.
    Missing,
    /// The signature is valid and the publisher is trusted.
    Verified {
        /// Publisher name.
        publisher: String,
    },
    /// The signature is valid but the publisher is not in the trust store.
    Untrusted {
        /// Publisher name.
        publisher: String,
        /// Why it is not trusted.
        reason: String,
    },
    /// The signature or its metadata is invalid.
    Invalid {
        /// Why verification failed.
        reason: String,
    },
}

impl SignatureVerification {
    /// Whether the signature proves origin and integrity.
    pub(crate) fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }

    /// A label for status rows.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Missing => "unsigned",
            Self::Verified { .. } => "verified",
            Self::Untrusted { .. } => "untrusted",
            Self::Invalid { .. } => "invalid",
        }
    }

    /// The trust level this verification justifies.
    pub(crate) fn trust_level(&self, fallback: TrustLevel) -> TrustLevel {
        match self {
            Self::Verified { .. } => TrustLevel::Verified,
            Self::Invalid { .. } => TrustLevel::Unknown,
            _ => fallback,
        }
    }
}

/// Verify a package signature against a trust store.
pub(crate) fn verify_package(
    root: &Path,
    id: &str,
    version: &str,
    hash: &str,
    trust: &TrustStore,
) -> SignatureVerification {
    let signature = match PackageSignature::load(root) {
        Ok(Some(signature)) => signature,
        Ok(None) => return SignatureVerification::Missing,
        Err(error) => {
            return SignatureVerification::Invalid {
                reason: error.to_string(),
            };
        }
    };
    if signature.schema_version != SIGNATURE_SCHEMA {
        return SignatureVerification::Invalid {
            reason: format!(
                "signature schema {:?} is not {SIGNATURE_SCHEMA}",
                signature.schema_version
            ),
        };
    }
    if signature.algorithm != SIGNATURE_ALGORITHM {
        return SignatureVerification::Invalid {
            reason: format!("algorithm {:?} is not Ed25519", signature.algorithm),
        };
    }
    let publisher = signature.publisher.clone();
    let Some(trusted_key) = trust.key(&publisher) else {
        return SignatureVerification::Untrusted {
            publisher,
            reason: "publisher is not in the trust store".to_string(),
        };
    };
    if trusted_key != signature.public_key {
        return SignatureVerification::Invalid {
            reason: "the package public key does not match the trusted key".to_string(),
        };
    }
    let public_key = match STANDARD
        .decode(&signature.public_key)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
    {
        Some(bytes) => VerifyingKey::from_bytes(&bytes),
        None => {
            return SignatureVerification::Invalid {
                reason: "public key is not 32 base64 bytes".to_string(),
            };
        }
    };
    let public_key = match public_key {
        Ok(key) => key,
        Err(error) => {
            return SignatureVerification::Invalid {
                reason: error.to_string(),
            };
        }
    };
    let bytes = match STANDARD
        .decode(&signature.signature)
        .ok()
        .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
    {
        Some(bytes) => bytes,
        None => {
            return SignatureVerification::Invalid {
                reason: "signature is not 64 base64 bytes".to_string(),
            };
        }
    };
    let signature = Signature::from_bytes(&bytes);
    let message = signed_message(id, version, hash);
    match public_key.verify_strict(&message, &signature) {
        Ok(()) => SignatureVerification::Verified { publisher },
        Err(error) => SignatureVerification::Invalid {
            reason: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bsh-ext-signing-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("tools")).expect("scratch");
        path
    }

    fn manifest(root: &Path) {
        std::fs::write(
            root.join("extension.toml"),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.signed"
name = "Signed"
version = "1.0.0"
description = "A signed test package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo"]
"#,
        )
        .expect("manifest");
        std::fs::write(root.join("tools/echo.json"), "{}").expect("tool");
    }

    #[test]
    fn a_signature_verifies_only_with_a_trusted_key() {
        let root = scratch("verify");
        manifest(&root);
        let (secret, public) = generate_key();
        let signature = sign_package(&root, "example", &secret).expect("sign");
        assert_eq!(signature.publisher, "example");
        let manifest = Manifest::load(&root).expect("manifest");
        let hash = package_hash(&root).expect("hash");

        let mut trust = TrustStore::load(root.join("trust.toml")).expect("trust");
        assert!(matches!(
            verify_package(&root, manifest.id().as_str(), "1.0.0", &hash, &trust),
            SignatureVerification::Untrusted { .. }
        ));
        trust.trust("example", &public, None).expect("trust");
        let verified = verify_package(&root, manifest.id().as_str(), "1.0.0", &hash, &trust);
        assert!(verified.is_verified(), "{verified:?}");
        assert_eq!(verified.trust_level(TrustLevel::Local), TrustLevel::Verified);

        std::fs::write(root.join("tools/echo.json"), "{\"changed\":true}").expect("tamper");
        let tampered_hash = package_hash(&root).expect("hash");
        assert!(matches!(
            verify_package(&root, manifest.id().as_str(), "1.0.0", &tampered_hash, &trust),
            SignatureVerification::Invalid { .. }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
