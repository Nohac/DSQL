//! The catalog as a bowl fact: a fingerprinted singleton snapshot.

use std::path::PathBuf;

use bowl::{Bowl, Component, Entity, Singleton};

use super::Catalog;

/// Filesystem root containing the YAML catalog sources for the active
/// project. Editor adapters read this bowl-owned fact when presenting a
/// semantic catalog definition as a protocol location.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct CatalogSourceRoot(pub PathBuf);

/// The schema catalog every context-aware check resolves against, as a
/// singleton component. The revision is the effective catalog's precomputed
/// semantic fingerprint. Constructing an equal catalog is fingerprint-neutral;
/// every consumer-visible provider or overlay fact must therefore participate
/// in [`Catalog::semantic_fingerprint`].
#[derive(Component)]
#[component(revision)]
pub struct CatalogSnapshot {
    catalog: Catalog,
    revision: u64,
}

impl CatalogSnapshot {
    pub fn new(catalog: Catalog) -> Self {
        let revision = catalog.semantic_fingerprint();
        Self { catalog, revision }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The effective semantic fingerprint used by the bowl.
    pub fn fingerprint(&self) -> u64 {
        self.revision
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
