//! Folder-backed resource (directory with listing and child access).

use std::path::{Path, PathBuf};

use super::{Resource, ResourceError, ResourceInfo, ResourceState, ResourceType};

/// A resource representing a directory on the host.
pub struct FolderResource {
    id: String,
    path: PathBuf,
    state: ResourceState,
}

impl FolderResource {
    /// Create a folder resource rooted at `path`.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let id = format!("folder://{}", path.display());
        Self {
            id,
            path,
            state: ResourceState::Closed,
        }
    }

    /// List immediate children (files and subdirectories).
    pub fn list(&self) -> Result<Vec<PathBuf>, ResourceError> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ResourceError::NotFound(self.path.display().to_string()),
            std::io::ErrorKind::PermissionDenied => {
                ResourceError::PermissionDenied(self.path.display().to_string())
            }
            _ => ResourceError::Io(e),
        })? {
            let entry = entry.map_err(ResourceError::Io)?;
            out.push(entry.path());
        }
        Ok(out)
    }

    /// Create the directory (and parents) on the host.
    pub fn ensure(&self) -> Result<(), ResourceError> {
        std::fs::create_dir_all(&self.path).map_err(ResourceError::Io)
    }
}

#[async_trait::async_trait]
impl Resource for FolderResource {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ResourceType {
        ResourceType::Folder
    }

    fn state(&self) -> ResourceState {
        self.state.clone()
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
    }

    async fn open(&mut self) -> Result<(), ResourceError> {
        if !self.path.exists() {
            self.ensure()?;
        }
        self.state = ResourceState::Open;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), ResourceError> {
        self.state = ResourceState::Closed;
        Ok(())
    }

    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, ResourceError> {
        Err(ResourceError::Unsupported)
    }

    async fn write(&mut self, _data: &[u8]) -> Result<usize, ResourceError> {
        Err(ResourceError::Unsupported)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl FolderResource {
    /// Convenience snapshot of current info.
    pub fn info(&self) -> ResourceInfo {
        super::Resource::info(self)
    }
}