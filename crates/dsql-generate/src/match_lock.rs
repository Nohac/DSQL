//! Storage-independent filter match-lock model assembled from resolved policy
//! facts. Native adapters own comparison and filesystem transactionality.

use std::collections::{BTreeMap, BTreeSet};

use dsql_core::catalog::Catalog;
use dsql_core::entities::policy::{CompiledPolicyIndex, PolicyIndex, PolicyKind};
use dsql_core::source::ScopeImports;
use facet::Facet;

/// Match-lock format version written by this compiler.
pub const FILTER_MATCH_LOCK_VERSION: u32 = 1;

/// Canonical semantic contents of `<project-root>/dsql/dsql.lock`.
#[derive(Clone, Debug, Facet, PartialEq, Eq)]
pub struct FilterMatchLock {
    pub version: u32,
    #[facet(default, skip_serializing_if = Vec::is_empty)]
    pub filters: Vec<LockedFilter>,
}

/// One filter identity as observed from one effective resolution scope.
#[derive(Clone, Debug, Facet, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedFilter {
    pub scope: String,
    pub defined_in: String,
    pub name: String,
    #[facet(default, skip_serializing_if = Vec::is_empty)]
    pub conditions: Vec<LockedPolicyReference>,
    pub matches: Vec<LockedFilterMatch>,
}

/// Resolved reusable-condition identity referenced by a filter.
#[derive(Clone, Debug, Facet, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedPolicyReference {
    pub scope: String,
    pub name: String,
}

/// One qualified catalog target and the structural fields that caused it to
/// match. Concrete targets omit `fields`.
#[derive(Clone, Debug, Facet, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedFilterMatch {
    pub target: String,
    #[facet(default, skip_serializing_if = BTreeMap::is_empty)]
    pub fields: BTreeMap<String, String>,
}

impl FilterMatchLock {
    /// Creates the canonical empty lock state. Native adapters represent it by
    /// an absent file rather than an empty document.
    pub fn empty() -> Self {
        Self {
            version: FILTER_MATCH_LOCK_VERSION,
            filters: Vec::new(),
        }
    }

    /// Whether no effective filter decisions need pinning.
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    /// Sorts and deduplicates all semantic collections before comparison or
    /// serialization.
    pub fn canonicalize(&mut self) {
        for filter in &mut self.filters {
            filter.conditions.sort();
            filter.conditions.dedup();
            filter.matches.sort();
            filter.matches.dedup();
        }
        self.filters.sort();
        self.filters.dedup();
    }

    /// Serializes the canonical lock as human-reviewable YAML.
    pub fn to_yaml(&self) -> Result<String, String> {
        let mut lock = self.clone();
        lock.canonicalize();
        let serialized = facet_yaml::to_string(&lock).map_err(|error| error.to_string())?;
        let mut canonical = serialized
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        if serialized.ends_with('\n') {
            canonical.push('\n');
        }
        Ok(canonical)
    }

    /// Parses and canonicalizes a lock document.
    pub fn from_yaml(input: &str) -> Result<Self, String> {
        let mut lock: Self = facet_yaml::from_str(input).map_err(|error| error.to_string())?;
        lock.canonicalize();
        Ok(lock)
    }

    /// Stable semantic lines used for concise stale-lock diagnostics.
    pub fn semantic_lines(&self) -> BTreeSet<String> {
        self.filters
            .iter()
            .flat_map(|filter| {
                let conditions = filter
                    .conditions
                    .iter()
                    .map(|condition| format!("{}::{}", condition.scope, condition.name))
                    .collect::<Vec<_>>()
                    .join(",");
                filter.matches.iter().map(move |matched| {
                    let fields = matched
                        .fields
                        .iter()
                        .map(|(name, data_type)| format!("{name}:{data_type}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!(
                        "{} <- {}::{}: {} [{}] conditions [{}]",
                        filter.scope,
                        filter.defined_in,
                        filter.name,
                        matched.target,
                        fields,
                        conditions,
                    )
                })
            })
            .collect()
    }
}

pub(crate) fn assemble_filter_match_lock(
    catalog: &Catalog,
    resolution: &PolicyIndex,
    compiled: &CompiledPolicyIndex,
    imports: &ScopeImports,
) -> FilterMatchLock {
    let mut scopes = imports.0.keys().cloned().collect::<BTreeSet<_>>();
    scopes.extend(resolution.entries.iter().map(|entry| entry.scope.clone()));

    let mut filters = Vec::new();
    for scope in scopes {
        let visible = imports.visible_from(&scope).collect::<BTreeSet<_>>();
        for entry in compiled
            .entries
            .iter()
            .filter(|entry| visible.contains(entry.scope.as_str()))
        {
            let Some(resolved) = resolution.entry(entry.entity) else {
                continue;
            };
            if resolved.kind != PolicyKind::Filter {
                continue;
            }
            let fields = resolved
                .target_fields
                .iter()
                .map(|(name, data_type)| (name.clone(), data_type.as_str().to_string()))
                .collect::<BTreeMap<_, _>>();
            let mut matches = entry
                .targets
                .iter()
                .filter_map(|target| {
                    catalog
                        .table_by_id(target.table)
                        .map(|table| LockedFilterMatch {
                            target: format!("{}.{}", table.schema, table.name),
                            fields: fields.clone(),
                        })
                })
                .collect::<Vec<_>>();
            matches.sort();
            matches.dedup();
            filters.push(LockedFilter {
                scope: scope.clone(),
                defined_in: entry.scope.clone(),
                name: entry.name.clone(),
                conditions: entry
                    .conditions
                    .iter()
                    .map(|condition| LockedPolicyReference {
                        scope: condition.scope.clone(),
                        name: condition.name.clone(),
                    })
                    .collect(),
                matches,
            });
        }
    }
    let mut lock = FilterMatchLock {
        version: FILTER_MATCH_LOCK_VERSION,
        filters,
    };
    lock.canonicalize();
    lock
}
