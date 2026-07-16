//! The catalog as a bowl fact: a fingerprinted singleton snapshot.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use bowl::{Bowl, Component, Entity, Singleton};

use super::Catalog;

/// Filesystem root containing the YAML catalog sources for the active
/// project. Editor adapters read this bowl-owned fact when presenting a
/// semantic catalog definition as a protocol location.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct CatalogSourceRoot(pub PathBuf);

/// The schema catalog every context-aware check resolves against, as a
/// singleton component. The fingerprint is the revision verbatim, drawn
/// from a process-global counter (the [`Catalog`] itself is large and
/// hashing it per settle would defeat the point); replacing the snapshot
/// always moves the revision, so checks that track it rerun exactly when
/// the catalog actually changes.
#[derive(Component)]
#[component(revision)]
pub struct CatalogSnapshot {
    catalog: Catalog,
    revision: u64,
}

static NEXT_REVISION: AtomicU64 = AtomicU64::new(0);

impl CatalogSnapshot {
    pub fn new(catalog: Catalog) -> Self {
        Self {
            catalog,
            revision: NEXT_REVISION.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

/// Installs (or replaces) the catalog snapshot singleton on the bowl.
pub async fn insert_catalog(bowl: &Bowl, catalog: Catalog) -> Entity {
    let inserted = bowl
        .insert((
            Singleton::<CatalogSnapshot>::new(),
            CatalogSnapshot::new(catalog),
        ))
        .await;
    inserted.entity()
}
