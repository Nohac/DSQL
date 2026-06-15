use crate::FileId;
use dsql_core::{
    DefinitionRecord, Diagnostic, DiagnosticCode, DiagnosticSource, FragmentRecord, QueryRecord,
    Severity, TextRange,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefinitionId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceRegionId(pub u32);

impl From<FileId> for SourceRegionId {
    fn from(value: FileId) -> Self {
        Self(value.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Root<T> {
    pub id: DefinitionId,
    pub source: SourceRegionId,
    pub ordinal: u32,
    pub name: Option<String>,
    pub name_range: Option<TextRange>,
    pub range: TextRange,
    pub inner: T,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueryRootInner {
    pub record: QueryRecord,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FragmentRootInner {
    pub record: FragmentRecord,
}

pub type QueryRoot = Root<QueryRootInner>;
pub type FragmentRoot = Root<FragmentRootInner>;

#[derive(Clone, Debug, PartialEq)]
pub enum RootDefinition {
    Query(QueryRoot),
    Fragment(FragmentRoot),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootDefinitionBucket {
    pub primary: DefinitionId,
    pub conflicts: Vec<DefinitionId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DefinitionIndex {
    pub definitions: Vec<RootDefinition>,
    pub named: HashMap<String, RootDefinitionBucket>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DefinitionIndex {
    pub fn from_records(source: impl Into<SourceRegionId>, records: Vec<DefinitionRecord>) -> Self {
        Self::from_sources([(source.into(), records)])
    }

    pub fn from_sources(
        sources: impl IntoIterator<Item = (SourceRegionId, Vec<DefinitionRecord>)>,
    ) -> Self {
        let mut index = Self::default();
        for (source, records) in sources {
            for (ordinal, record) in records.into_iter().enumerate() {
                index.push_record(source, ordinal as u32, record);
            }
        }
        index
    }

    pub fn definition(&self, id: DefinitionId) -> Option<&RootDefinition> {
        self.definitions.get(id.0 as usize)
    }

    pub fn get(&self, name: &str) -> Option<&RootDefinition> {
        self.definition(self.bucket(name)?.primary)
    }

    pub fn bucket(&self, name: &str) -> Option<&RootDefinitionBucket> {
        self.named.get(name)
    }

    pub fn queries(&self) -> impl Iterator<Item = &QueryRoot> {
        self.definitions.iter().filter_map(RootDefinition::as_query)
    }

    pub fn fragments(&self) -> impl Iterator<Item = &FragmentRoot> {
        self.definitions
            .iter()
            .filter_map(RootDefinition::as_fragment)
    }

    pub fn queries_by_name(&self, name: &str) -> impl Iterator<Item = &QueryRoot> + '_ {
        self.definition_ids_by_name(name)
            .into_iter()
            .filter_map(|id| self.definition(id))
            .filter_map(RootDefinition::as_query)
    }

    pub fn fragments_by_name(&self, name: &str) -> impl Iterator<Item = &FragmentRoot> + '_ {
        self.definition_ids_by_name(name)
            .into_iter()
            .filter_map(|id| self.definition(id))
            .filter_map(RootDefinition::as_fragment)
    }

    fn push_record(&mut self, source: SourceRegionId, ordinal: u32, record: DefinitionRecord) {
        let id = DefinitionId(self.definitions.len() as u32);
        let definition = match record {
            DefinitionRecord::Query(record) => RootDefinition::Query(QueryRoot {
                id,
                source,
                ordinal,
                name: record.key.name.clone(),
                name_range: record.name_range,
                range: record.range,
                inner: QueryRootInner { record },
            }),
            DefinitionRecord::Fragment(record) => RootDefinition::Fragment(FragmentRoot {
                id,
                source,
                ordinal,
                name: Some(record.key.name.clone()),
                name_range: Some(record.name_range),
                range: record.range,
                inner: FragmentRootInner { record },
            }),
        };

        if let Some(name) = definition.name() {
            if let Some(bucket) = self.named.get_mut(name) {
                bucket.conflicts.push(id);
                self.diagnostics
                    .push(duplicate_definition_diagnostic(name, &definition));
            } else {
                self.named.insert(
                    name.to_string(),
                    RootDefinitionBucket {
                        primary: id,
                        conflicts: Vec::new(),
                    },
                );
            }
        }
        self.definitions.push(definition);
    }

    fn definition_ids_by_name(&self, name: &str) -> Vec<DefinitionId> {
        let Some(bucket) = self.bucket(name) else {
            return Vec::new();
        };
        std::iter::once(bucket.primary)
            .chain(bucket.conflicts.iter().copied())
            .collect()
    }
}

impl RootDefinition {
    pub fn id(&self) -> DefinitionId {
        match self {
            Self::Query(root) => root.id,
            Self::Fragment(root) => root.id,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Query(root) => root.name.as_deref(),
            Self::Fragment(root) => root.name.as_deref(),
        }
    }

    pub fn name_range(&self) -> Option<TextRange> {
        match self {
            Self::Query(root) => root.name_range,
            Self::Fragment(root) => root.name_range,
        }
    }

    pub fn range(&self) -> TextRange {
        match self {
            Self::Query(root) => root.range,
            Self::Fragment(root) => root.range,
        }
    }

    pub fn as_query(&self) -> Option<&QueryRoot> {
        match self {
            Self::Query(root) => Some(root),
            Self::Fragment(_) => None,
        }
    }

    pub fn as_fragment(&self) -> Option<&FragmentRoot> {
        match self {
            Self::Query(_) => None,
            Self::Fragment(root) => Some(root),
        }
    }
}

fn duplicate_definition_diagnostic(name: &str, definition: &RootDefinition) -> Diagnostic {
    Diagnostic {
        range: definition
            .name_range()
            .unwrap_or_else(|| definition.range()),
        severity: Severity::Error,
        code: DiagnosticCode::DuplicateDefinition,
        source: DiagnosticSource::Check,
        message: format!("duplicate definition `{name}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsql_core::{SourceSnapshot, extract_definitions, parse_source};

    #[test]
    fn definition_index_preserves_roots_and_records_named_conflicts() {
        let parsed = parse_source(SourceSnapshot::from(
            "query First { users { id } }\n\
             query { users { id } }\n\
             fragment Shared on users { id }\n\
             fragment Shared on users { name }\n",
        ));
        let extracted = extract_definitions(&parsed.source_file);

        let index = DefinitionIndex::from_records(FileId(7), extracted.definitions);

        assert_eq!(
            index
                .definitions
                .iter()
                .filter_map(RootDefinition::name)
                .collect::<Vec<_>>(),
            vec!["First", "Shared", "Shared"]
        );
        assert_eq!(index.queries().count(), 2);
        assert_eq!(index.fragments().count(), 2);
        assert!(index.bucket("First").is_some());
        assert_eq!(index.bucket("Shared").unwrap().conflicts.len(), 1);
        assert_eq!(index.fragments_by_name("Shared").count(), 2);
        assert_eq!(index.diagnostics.len(), 1);
        assert_eq!(
            index.diagnostics[0].range,
            index
                .fragments_by_name("Shared")
                .last()
                .unwrap()
                .name_range
                .unwrap()
        );
    }
}
