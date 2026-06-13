use crate::syntax::{
    Definition, FragmentDef, QueryDef, Selection, SelectionKind, SourceFile, TextRange,
};
use facet::Facet;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct QueryKey {
    pub name: Option<String>,
    pub ordinal: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct FragmentKey {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct ExtractedFile {
    pub definitions: Vec<DefinitionRecord>,
}

#[derive(Clone, Debug, PartialEq, Facet)]
#[repr(C)]
pub enum DefinitionRecord {
    Query(QueryRecord),
    Fragment(FragmentRecord),
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct QueryRecord {
    pub key: QueryKey,
    pub range: TextRange,
    pub name_range: Option<TextRange>,
    pub selections: Vec<Selection>,
    pub fragment_spreads: Vec<FragmentSpreadRef>,
}

#[derive(Clone, Debug, PartialEq, Facet)]
pub struct FragmentRecord {
    pub key: FragmentKey,
    pub range: TextRange,
    pub name_range: TextRange,
    pub on: Option<String>,
    pub on_range: Option<TextRange>,
    pub selections: Vec<Selection>,
    pub fragment_spreads: Vec<FragmentSpreadRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct FragmentSpreadRef {
    pub name: String,
    pub range: TextRange,
}

pub trait DefinitionResolver {
    fn fragment(&self, name: &str) -> Option<&FragmentRecord>;
}

#[derive(Clone, Debug, Default)]
pub struct FragmentMap {
    fragments: HashMap<String, Vec<FragmentRecord>>,
}

impl FragmentMap {
    pub fn from_file(file: &ExtractedFile) -> Self {
        let mut fragments = Self::default();
        for definition in &file.definitions {
            if let DefinitionRecord::Fragment(fragment) = definition {
                fragments.insert(fragment.clone());
            }
        }
        fragments
    }

    pub fn insert(&mut self, fragment: FragmentRecord) {
        self.fragments
            .entry(fragment.key.name.clone())
            .or_default()
            .push(fragment);
    }

    pub fn duplicate_fragments(&self) -> Vec<&FragmentRecord> {
        let mut duplicates = self
            .fragments
            .values()
            .filter(|fragments| fragments.len() > 1)
            .flat_map(|fragments| fragments.iter().skip(1))
            .collect::<Vec<_>>();
        duplicates.sort_by_key(|fragment| (fragment.name_range.start, fragment.name_range.end));
        duplicates
    }
}

impl DefinitionResolver for FragmentMap {
    fn fragment(&self, name: &str) -> Option<&FragmentRecord> {
        self.fragments
            .get(name)
            .and_then(|fragments| fragments.first())
    }
}

pub fn extract_definitions(source_file: &SourceFile) -> ExtractedFile {
    let mut query_ordinal = 0;
    let definitions = source_file
        .definitions()
        .map(|definition| match definition {
            Definition::Query(query) => {
                let record = query_record(query, query_ordinal);
                query_ordinal += 1;
                DefinitionRecord::Query(record)
            }
            Definition::Fragment(fragment) => DefinitionRecord::Fragment(fragment_record(fragment)),
        })
        .collect();
    ExtractedFile { definitions }
}

fn query_record(query: &QueryDef, ordinal: u32) -> QueryRecord {
    QueryRecord {
        key: QueryKey {
            name: query.name.as_ref().map(|name| name.text.clone()),
            ordinal,
        },
        range: query.range,
        name_range: query.name.as_ref().map(|name| name.range),
        fragment_spreads: fragment_spreads(&query.selections),
        selections: query.selections.clone(),
    }
}

fn fragment_record(fragment: &FragmentDef) -> FragmentRecord {
    let name = fragment
        .name
        .as_ref()
        .map_or_else(|| "<missing>".to_string(), |name| name.text.clone());
    FragmentRecord {
        key: FragmentKey { name },
        range: fragment.range,
        name_range: fragment
            .name
            .as_ref()
            .map_or(fragment.range, |name| name.range),
        on: fragment.on.as_ref().map(|on| on.text.clone()),
        on_range: fragment.on.as_ref().map(|on| on.range),
        fragment_spreads: fragment_spreads(&fragment.selections),
        selections: fragment.selections.clone(),
    }
}

fn fragment_spreads(selections: &[Selection]) -> Vec<FragmentSpreadRef> {
    let mut spreads = Vec::new();
    collect_fragment_spreads(selections, &mut spreads);
    spreads
}

fn collect_fragment_spreads(selections: &[Selection], spreads: &mut Vec<FragmentSpreadRef>) {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            spreads.push(FragmentSpreadRef {
                name: selection.name.text.clone(),
                range: selection.name.range,
            });
        }
        collect_fragment_spreads(&selection.selections, spreads);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SourceSnapshot, parse_source};

    #[test]
    fn fragment_map_preserves_duplicate_fragments_for_diagnostics() {
        let parsed = parse_source(SourceSnapshot::from_string(
            r#"
fragment MovieFields on movie_info {
  id
}

fragment MovieFields on movie_info {
  info
}
"#
            .to_string(),
        ));
        let extracted = extract_definitions(&parsed.source_file);
        let fragments = FragmentMap::from_file(&extracted);

        assert_eq!(
            fragments
                .fragment("MovieFields")
                .map(|fragment| fragment.selections[0].name.text.as_str()),
            Some("id")
        );
        assert_eq!(fragments.duplicate_fragments().len(), 1);
        assert_eq!(
            fragments.duplicate_fragments()[0].selections[0]
                .name
                .text
                .as_str(),
            "info"
        );
    }
}
