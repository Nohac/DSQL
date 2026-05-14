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
    fragments: HashMap<String, FragmentRecord>,
}

impl FragmentMap {
    pub fn from_file(file: &ExtractedFile) -> Self {
        let fragments = file
            .definitions
            .iter()
            .filter_map(|definition| match definition {
                DefinitionRecord::Query(_) => None,
                DefinitionRecord::Fragment(fragment) => {
                    Some((fragment.key.name.clone(), fragment.clone()))
                }
            })
            .collect();
        Self { fragments }
    }

    pub fn insert(&mut self, fragment: FragmentRecord) {
        self.fragments.insert(fragment.key.name.clone(), fragment);
    }
}

impl DefinitionResolver for FragmentMap {
    fn fragment(&self, name: &str) -> Option<&FragmentRecord> {
        self.fragments.get(name)
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
