//! Fragment-spread expansion as one shared decision: which fragment a
//! spread splices in, in what order, and where cycles cut off. Checks,
//! variable inference, planning, and output-key validation all expand
//! through this walker so their behavior cannot drift — provenance stays
//! with the caller, which knows both the spread site and the fragment.

use bowl::Entity;

use crate::entities::field_selection::SelectionTree;
use crate::source::ScopeImports;

/// One expansion walk: resolves spread names through the effective
/// resolver and tracks the fragment path for cycle cutoff. Callers pair
/// every [`SpreadExpansion::enter`] that returned a fragment with one
/// [`SpreadExpansion::leave`].
pub(crate) struct SpreadExpansion<'t, 'v> {
    tree: &'t SelectionTree<'v>,
    scope: &'t str,
    imports: &'t ScopeImports,
    visiting: Vec<String>,
}

/// What one spread name expands to.
pub(crate) enum ExpandedSpread {
    /// The fragment is already on the expansion path: a cycle. Not
    /// entered; do not `leave`.
    Cycle,
    /// No unique visible fragment; the spread checks report why. Not
    /// entered; do not `leave`.
    Unresolved,
    /// The unique visible fragment; entered onto the path.
    Fragment { entity: Entity },
}

impl<'t, 'v> SpreadExpansion<'t, 'v> {
    pub(crate) fn new(
        tree: &'t SelectionTree<'v>,
        scope: &'t str,
        imports: &'t ScopeImports,
    ) -> Self {
        Self {
            tree,
            scope,
            imports,
            visiting: Vec::new(),
        }
    }

    /// Resolves `name` for expansion and pushes it onto the path when it
    /// yields a fragment.
    pub(crate) fn enter(&mut self, name: &str) -> ExpandedSpread {
        if self.visiting.iter().any(|visited| visited == name) {
            return ExpandedSpread::Cycle;
        }
        let Some((entity, _, _, _)) = self
            .tree
            .resolve_fragment(name, self.scope, self.imports)
            .copied()
        else {
            return ExpandedSpread::Unresolved;
        };
        self.visiting.push(name.to_string());
        ExpandedSpread::Fragment { entity }
    }

    /// Pops the most recent fragment off the expansion path.
    pub(crate) fn leave(&mut self) {
        self.visiting.pop();
    }

    /// Seeds the path with an enclosing fragment, so a body's spreads
    /// treat their own fragment as already entered (its cycle diagnostics
    /// belong to the cycle check, not to every consumer).
    pub(crate) fn seed(&mut self, name: &str) {
        self.visiting.push(name.to_string());
    }
}
