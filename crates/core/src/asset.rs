use std::{
    any::{Any, TypeId},
    collections::HashMap,
};

/// Assets available to atom-dispatched compiler stages.
///
/// Project assets live for the request, project, or revision that prepared the
/// stage. Atom assets are recreated for one pass and can be drained by the
/// owning stage after dispatch completes.
#[derive(Default)]
pub struct AssetRegistry {
    /// Shared assets prepared before a stage pass.
    pub project: ProjectAssets,
    /// Fresh per-pass assets owned by one stage invocation.
    pub atom: AtomAssets,
}

impl AssetRegistry {
    /// Returns a project asset by concrete type.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.project.get()
    }

    /// Returns a mutable project asset by concrete type.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.project.get_mut()
    }
}

/// Long-lived assets prepared before an atom-dispatched stage pass.
#[derive(Default)]
pub struct ProjectAssets {
    assets: HashMap<TypeId, Box<dyn Any>>,
}

impl ProjectAssets {
    /// Inserts or replaces a project asset of the same concrete type.
    pub fn insert<T: 'static>(&mut self, asset: T) -> Option<T> {
        self.assets
            .insert(TypeId::of::<T>(), Box::new(asset))
            .and_then(|asset| asset.downcast::<T>().ok())
            .map(|asset| *asset)
    }

    /// Returns a project asset by concrete type.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.assets
            .get(&TypeId::of::<T>())
            .and_then(|asset| asset.downcast_ref())
    }

    /// Returns a mutable project asset by concrete type.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.assets
            .get_mut(&TypeId::of::<T>())
            .and_then(|asset| asset.downcast_mut())
    }
}

/// Fresh per-pass assets owned by one atom-dispatched stage invocation.
#[derive(Default)]
pub struct AtomAssets {
    assets: HashMap<TypeId, Box<dyn Any>>,
}

impl AtomAssets {
    /// Inserts or replaces an atom asset of the same concrete type.
    pub fn insert<T: 'static>(&mut self, asset: T) -> Option<T> {
        self.assets
            .insert(TypeId::of::<T>(), Box::new(asset))
            .and_then(|asset| asset.downcast::<T>().ok())
            .map(|asset| *asset)
    }

    /// Returns an atom asset by concrete type.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.assets
            .get(&TypeId::of::<T>())
            .and_then(|asset| asset.downcast_ref())
    }

    /// Returns a mutable atom asset by concrete type.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.assets
            .get_mut(&TypeId::of::<T>())
            .and_then(|asset| asset.downcast_mut())
    }

    /// Removes and returns an atom asset by concrete type.
    pub fn take<T: 'static>(&mut self) -> Option<T> {
        self.assets
            .remove(&TypeId::of::<T>())
            .and_then(|asset| asset.downcast::<T>().ok())
            .map(|asset| *asset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct ProjectValue(&'static str);

    #[derive(Debug, PartialEq, Eq)]
    struct AtomValue(u8);

    #[test]
    fn project_asset_can_be_inserted_and_extracted_by_type() {
        let mut assets = ProjectAssets::default();

        assert!(assets.get::<ProjectValue>().is_none());
        assets.insert(ProjectValue("registry"));

        assert_eq!(
            assets.get::<ProjectValue>(),
            Some(&ProjectValue("registry"))
        );
    }

    #[test]
    fn atom_asset_can_be_taken_once_by_type() {
        let mut assets = AtomAssets::default();

        assets.insert(AtomValue(7));

        assert_eq!(assets.take::<AtomValue>(), Some(AtomValue(7)));
        assert!(assets.take::<AtomValue>().is_none());
        assert!(assets.get::<AtomValue>().is_none());
    }
}
