//! The catalog as a bowl fact: a fingerprinted singleton snapshot.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use bowl::{Bowl, Component, Entity, Singleton};

use super::Catalog;

/// The schema catalog every context-aware check resolves against, as a
/// singleton component. The fingerprint is a revision from a process-global
/// counter (the [`Catalog`] itself is large and hashing it per settle would
/// defeat the point); replacing the snapshot always moves the revision, so
/// checks that track it rerun exactly when the catalog actually changes.
#[derive(Component)]
#[component(hash)]
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

impl Hash for CatalogSnapshot {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.revision.hash(state);
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
