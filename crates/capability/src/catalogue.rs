//! The metadata authority: one entry per capability id, plus the surface index.
//!
//! A catalogue is not a dispatch registry. It answers what exists and what it
//! promises; it never decides who may call it.

use crate::descriptor::CapabilityDescriptor;
use crate::error::CapabilityError;
use crate::id::{CapabilityId, SurfaceName};
use std::collections::{BTreeMap, BTreeSet};

/// Descriptors keyed by capability id, with a unique surface-name index.
#[derive(Debug, Clone, Default)]
pub struct Catalogue {
    entries: BTreeMap<CapabilityId, CapabilityDescriptor>,
    surfaces: BTreeMap<SurfaceName, CapabilityId>,
}

impl Catalogue {
    /// An empty catalogue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one descriptor.
    ///
    /// A duplicate id and a duplicate surface name are both refused: two
    /// capabilities answering to one surface name make dispatch ambiguous.
    pub fn insert(&mut self, descriptor: CapabilityDescriptor) -> Result<(), CapabilityError> {
        let id = descriptor.id.clone();
        let surface = descriptor.surface.clone();
        if self.entries.contains_key(&id) {
            return Err(CapabilityError::Invalid {
                message: format!("duplicate capability id: {id}"),
            });
        }
        if let Some(owner) = self.surfaces.get(&surface) {
            return Err(CapabilityError::Invalid {
                message: format!("surface name {surface} is already used by {owner}"),
            });
        }
        self.surfaces.insert(surface, id.clone());
        self.entries.insert(id, descriptor);
        Ok(())
    }

    /// One descriptor by id.
    pub fn get(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.entries.get(id)
    }

    /// One descriptor by the surface name a model calls.
    pub fn by_surface(&self, surface: &SurfaceName) -> Option<&CapabilityDescriptor> {
        self.surfaces
            .get(surface)
            .and_then(|id| self.entries.get(id))
    }

    /// Every descriptor.
    pub fn iter(&self) -> impl Iterator<Item = &CapabilityDescriptor> {
        self.entries.values()
    }

    /// Every id.
    pub fn ids(&self) -> impl Iterator<Item = &CapabilityId> {
        self.entries.keys()
    }

    /// Every surface name in use.
    pub fn surface_names(&self) -> BTreeSet<&SurfaceName> {
        self.surfaces.keys().collect()
    }

    /// How many capabilities the catalogue holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalogue is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
