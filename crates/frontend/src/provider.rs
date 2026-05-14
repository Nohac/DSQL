use dsql_core::Catalog;

pub trait CatalogProvider {
    fn load_catalog(&self) -> Catalog;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HardcodedCatalogProvider;

impl CatalogProvider for HardcodedCatalogProvider {
    fn load_catalog(&self) -> Catalog {
        Catalog::hardcoded()
    }
}
