use crate::analysis::collect_diagnostics_parts;
use crate::completion::CompletionScope;
use crate::document::{FileId, RevisionId};
use dashmap::DashMap;
use dsql_core::{
    Catalog, CheckDiagnostic, CheckDiagnosticKind, CheckedFile, CompilerDiagnostic,
    CompilerDiagnosticSource, DefinitionRecord, Diagnostic, ExtractedFile, FragmentMap,
    FragmentRecord, Interner, LintOptions, LintedFile, LoweredFile, ParseResult, PlannedFile,
    QueryRecord, SourceFile, SourceSnapshot, SyntaxTree, check_file_with_catalog,
    check_fragment_definition, check_query_definition, extract_definitions, format_file,
    lint_file_with_options, lint_fragment_definition_with_options,
    lint_query_definition_with_options, lower_file, parse_source, plan_file_with_catalog,
    plan_query_definition,
};
use facet::Facet;
use picante::PicanteResult;
use ropey::Rope;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct AnalysisResult {
    pub parse: ParseResult,
    pub lower: LoweredFile,
    pub check: CheckedFile,
    pub lint: LintedFile,
    pub plan: PlannedFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FragmentDefinitionLocation {
    pub file: FileId,
    pub range: dsql_core::TextRange,
}

#[derive(Clone, Debug, Facet)]
pub struct ParsedFile {
    pub tree: SyntaxTree,
    pub source_file: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

impl CompilerDiagnosticSource for ParsedFile {
    fn extend_compiler_diagnostics(&self, diagnostics: &mut Vec<CompilerDiagnostic>) {
        diagnostics.extend(
            self.diagnostics
                .iter()
                .cloned()
                .map(CompilerDiagnostic::from),
        );
    }
}

#[picante::input]
pub struct SourceInput {
    #[key]
    pub file: FileId,
    pub revision: RevisionId,
    pub text: Arc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
pub struct ContextSource {
    pub file: FileId,
    pub source: SourceInput,
}

#[derive(Clone, Debug, Facet)]
pub struct ContextDefinitionFile {
    pub file: FileId,
    pub definitions: Vec<DefinitionRecord>,
}

#[derive(Clone, Debug, Facet)]
pub struct ContextDefinitions {
    pub files: Vec<ContextDefinitionFile>,
}

#[picante::input]
pub struct CatalogInput {
    pub catalog: Catalog,
}

#[picante::input]
pub struct LintOptionsInput {
    pub options: LintOptions,
}

#[picante::input]
pub struct ContextSourcesInput {
    #[key]
    pub context: String,
    pub sources: Vec<ContextSource>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
pub struct QueryDefinitionKey {
    pub file: FileId,
    pub ordinal: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Facet)]
pub struct FragmentDefinitionKey {
    pub file: FileId,
    pub ordinal: u32,
    pub name: String,
}

#[picante::input]
pub struct FragmentDefinitionInput {
    #[key]
    pub key: FragmentDefinitionKey,
    pub name: String,
    pub record: Option<FragmentRecord>,
    pub fragments: Vec<FragmentDefinitionInput>,
}

#[picante::input]
pub struct CompletionScopeInput {
    #[key]
    pub file: FileId,
    pub fragments: Vec<FragmentDefinitionInput>,
}

#[picante::input]
pub struct QueryDefinitionInput {
    #[key]
    pub key: QueryDefinitionKey,
    pub record: Option<QueryRecord>,
    pub fragments: Vec<FragmentDefinitionInput>,
}

#[picante::tracked]
pub async fn completion_scope_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    input: CompletionScopeInput,
) -> PicanteResult<CompletionScope> {
    let mut fragments = Vec::new();
    let mut seen = HashSet::<String>::new();
    for fragment_input in input.fragments(db)? {
        let name = fragment_input.name(db)?.clone();
        if !seen.insert(name) {
            continue;
        }
        if let Some(record) = fragment_input.record(db)? {
            fragments.push(record);
        }
    }
    fragments.sort_by(|left, right| left.key.name.cmp(&right.key.name));
    Ok(CompletionScope { fragments })
}

#[picante::tracked]
pub async fn parse_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<ParsedFile> {
    let text = source.text(db)?;
    let parse = parse_source(SourceSnapshot::from_arc(text));
    Ok(ParsedFile {
        tree: parse.tree,
        source_file: parse.source_file,
        diagnostics: parse.diagnostics,
    })
}

#[picante::tracked]
pub async fn lower_file_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<LoweredFile> {
    let parse = parse_file(db, source).await?;
    let mut interner = Interner::default();
    Ok(lower_file(&parse.source_file, &mut interner))
}

#[picante::tracked]
pub async fn extract_definitions_for_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<ExtractedFile> {
    let parse = parse_file(db, source).await?;
    Ok(extract_definitions(&parse.source_file))
}

#[picante::tracked]
pub async fn context_definitions_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    context: ContextSourcesInput,
) -> PicanteResult<ContextDefinitions> {
    let mut files = Vec::new();
    for source in context.sources(db)? {
        let extracted = extract_definitions_for_file(db, source.source).await?;
        files.push(ContextDefinitionFile {
            file: source.file,
            definitions: extracted.definitions,
        });
    }
    Ok(ContextDefinitions { files })
}

#[picante::tracked]
pub async fn check_file_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<CheckedFile> {
    let parse = parse_file(db, source).await?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    Ok(check_file_with_catalog(&parse.source_file, &catalog))
}

#[picante::tracked]
pub async fn plan_file_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<PlannedFile> {
    let parse = parse_file(db, source).await?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    Ok(plan_file_with_catalog(&parse.source_file, &catalog))
}

#[picante::tracked]
pub async fn lint_file_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<LintedFile> {
    let parse = parse_file(db, source).await?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    let options = LintOptionsInput::options(db)?.unwrap_or_default();
    Ok(lint_file_with_options(
        &parse.source_file,
        &catalog,
        options,
    ))
}

#[picante::tracked]
pub async fn check_query_definition_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    input: QueryDefinitionInput,
) -> PicanteResult<CheckedFile> {
    let Some(record) = input.record(db)? else {
        return Ok(empty_checked());
    };
    let fragments = resolve_fragments(db, input.fragments(db)?)?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    Ok(check_query_definition(&record, &fragments, &catalog))
}

#[picante::tracked]
pub async fn check_fragment_definition_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    input: FragmentDefinitionInput,
) -> PicanteResult<CheckedFile> {
    let Some(record) = input.record(db)? else {
        return Ok(empty_checked());
    };
    let fragments = resolve_fragments(db, input.fragments(db)?)?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    Ok(check_fragment_definition(&record, &fragments, &catalog))
}

#[picante::tracked]
pub async fn lint_query_definition_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    input: QueryDefinitionInput,
) -> PicanteResult<LintedFile> {
    let Some(record) = input.record(db)? else {
        return Ok(empty_linted());
    };
    let fragments = resolve_fragments(db, input.fragments(db)?)?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    let options = LintOptionsInput::options(db)?.unwrap_or_default();
    Ok(lint_query_definition_with_options(
        &record, &fragments, &catalog, options,
    ))
}

#[picante::tracked]
pub async fn lint_fragment_definition_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    input: FragmentDefinitionInput,
) -> PicanteResult<LintedFile> {
    let Some(record) = input.record(db)? else {
        return Ok(empty_linted());
    };
    let fragments = resolve_fragments(db, input.fragments(db)?)?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    let options = LintOptionsInput::options(db)?.unwrap_or_default();
    Ok(lint_fragment_definition_with_options(
        &record, &fragments, &catalog, options,
    ))
}

#[picante::tracked]
pub async fn plan_query_definition_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    input: QueryDefinitionInput,
) -> PicanteResult<PlannedFile> {
    let Some(record) = input.record(db)? else {
        return Ok(empty_planned());
    };
    let fragments = resolve_fragments(db, input.fragments(db)?)?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    Ok(plan_query_definition(&record, &fragments, &catalog))
}

#[picante::tracked]
pub async fn diagnostics_for_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<Vec<Diagnostic>> {
    let parse = parse_file(db, source).await?;
    let lower = lower_file_query(db, source).await?;
    let check = check_file_query(db, source).await?;
    let lint = lint_file_query(db, source).await?;
    let plan = plan_file_query(db, source).await?;
    Ok(collect_diagnostics_parts(&[
        &parse, &lower, &check, &lint, &plan,
    ]))
}

#[picante::tracked]
pub async fn diagnostics_for_file_in_context<DB: CompilerDatabaseTrait>(
    db: &DB,
    context: ContextSourcesInput,
    file: FileId,
    source: SourceInput,
) -> PicanteResult<Vec<Diagnostic>> {
    let parse = parse_file(db, source).await?;
    let lower = lower_file_query(db, source).await?;
    let definitions = context_definitions_query(db, context).await?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    let options = LintOptionsInput::options(db)?.unwrap_or_default();
    let check = context_check_file(&definitions, file, &catalog);
    let lint = context_lint_file(&definitions, file, &catalog, options);
    let plan = context_plan_file(&definitions, file, &catalog);
    Ok(collect_diagnostics_parts(&[
        &parse, &lower, &check, &lint, &plan,
    ]))
}

#[picante::tracked]
pub async fn formatted_text_for_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<Option<String>> {
    let parsed = parse_file(db, source).await?;
    let text = source.text(db)?;
    let parse = ParseResult {
        source: SourceSnapshot::from_arc(text),
        tree: parsed.tree,
        source_file: parsed.source_file,
        diagnostics: parsed.diagnostics,
    };
    let formatted = format_file(&parse);
    if formatted.diagnostics.is_empty() {
        Ok(Some(formatted.text))
    } else {
        Ok(None)
    }
}

fn resolve_fragments(
    db: &impl CompilerDatabaseTrait,
    inputs: Vec<FragmentDefinitionInput>,
) -> PicanteResult<FragmentMap> {
    let mut fragments = FragmentMap::default();
    let mut seen = HashSet::<String>::new();
    let mut stack = inputs;
    while let Some(input) = stack.pop() {
        let name = input.name(db)?.clone();
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(record) = input.record(db)? else {
            continue;
        };
        stack.extend(input.fragments(db)?);
        fragments.insert(record);
    }
    Ok(fragments)
}

fn context_fragment_map(definitions: &ContextDefinitions) -> FragmentMap {
    let mut fragments = FragmentMap::default();
    for file in &definitions.files {
        for definition in &file.definitions {
            if let DefinitionRecord::Fragment(fragment) = definition {
                fragments.insert(fragment.clone());
            }
        }
    }
    fragments
}

fn context_check_file(
    definitions: &ContextDefinitions,
    file: FileId,
    catalog: &Catalog,
) -> CheckedFile {
    let resolver = context_fragment_map(definitions);
    let mut diagnostics = duplicate_fragment_diagnostics_for_file(definitions, file);
    for definition in definitions_for_file(definitions, file) {
        match definition {
            DefinitionRecord::Query(query) => {
                diagnostics.extend(check_query_definition(query, &resolver, catalog).diagnostics);
            }
            DefinitionRecord::Fragment(fragment) => {
                diagnostics
                    .extend(check_fragment_definition(fragment, &resolver, catalog).diagnostics);
            }
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    CheckedFile {
        errors: diagnostics.clone(),
        diagnostics,
    }
}

fn context_lint_file(
    definitions: &ContextDefinitions,
    file: FileId,
    catalog: &Catalog,
    options: LintOptions,
) -> LintedFile {
    let resolver = context_fragment_map(definitions);
    let mut diagnostics = Vec::new();
    for definition in definitions_for_file(definitions, file) {
        match definition {
            DefinitionRecord::Query(query) => diagnostics.extend(
                lint_query_definition_with_options(query, &resolver, catalog, options).diagnostics,
            ),
            DefinitionRecord::Fragment(fragment) => diagnostics.extend(
                lint_fragment_definition_with_options(fragment, &resolver, catalog, options)
                    .diagnostics,
            ),
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    LintedFile { diagnostics }
}

fn context_plan_file(
    definitions: &ContextDefinitions,
    file: FileId,
    catalog: &Catalog,
) -> PlannedFile {
    let resolver = context_fragment_map(definitions);
    let mut queries = Vec::new();
    let mut diagnostics = Vec::new();
    for definition in definitions_for_file(definitions, file) {
        if let DefinitionRecord::Query(query) = definition {
            let plan = plan_query_definition(query, &resolver, catalog);
            queries.extend(plan.queries);
            diagnostics.extend(plan.diagnostics);
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    PlannedFile {
        queries,
        diagnostics,
    }
}

fn definitions_for_file(
    definitions: &ContextDefinitions,
    file: FileId,
) -> impl Iterator<Item = &DefinitionRecord> {
    definitions
        .files
        .iter()
        .filter(move |definition_file| definition_file.file == file)
        .flat_map(|definition_file| definition_file.definitions.iter())
}

fn duplicate_fragment_diagnostics_for_file(
    definitions: &ContextDefinitions,
    file: FileId,
) -> Vec<CheckDiagnostic> {
    let mut fragments = Vec::<(String, FileId, FragmentRecord)>::new();
    for definition_file in &definitions.files {
        for definition in &definition_file.definitions {
            if let DefinitionRecord::Fragment(fragment) = definition {
                fragments.push((
                    fragment.key.name.clone(),
                    definition_file.file,
                    fragment.clone(),
                ));
            }
        }
    }

    let mut diagnostics = Vec::new();
    let mut seen = HashSet::<String>::new();
    for (name, fragment_file, fragment) in fragments {
        if seen.insert(name.clone()) {
            continue;
        }
        if fragment_file == file {
            diagnostics.push(CheckDiagnostic {
                range: fragment.name_range,
                kind: CheckDiagnosticKind::DuplicateFragment { name },
            });
        }
    }
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    diagnostics
}

fn empty_checked() -> CheckedFile {
    CheckedFile {
        errors: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn empty_linted() -> LintedFile {
    LintedFile {
        diagnostics: Vec::new(),
    }
}

fn empty_planned() -> PlannedFile {
    PlannedFile {
        queries: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[picante::db(
    inputs(
        SourceInput,
        CatalogInput,
        LintOptionsInput,
        ContextSourcesInput,
        QueryDefinitionInput,
        FragmentDefinitionInput,
        CompletionScopeInput
    ),
    tracked(
        completion_scope_query,
        parse_file,
        lower_file_query,
        extract_definitions_for_file,
        context_definitions_query,
        check_file_query,
        plan_file_query,
        lint_file_query,
        check_query_definition_query,
        check_fragment_definition_query,
        lint_query_definition_query,
        lint_fragment_definition_query,
        plan_query_definition_query,
        diagnostics_for_file,
        diagnostics_for_file_in_context,
        formatted_text_for_file
    ),
    db_trait(CompilerDatabaseTrait)
)]
pub struct CompilerDb {
    source_inputs: DashMap<FileId, SourceInput>,
    query_inputs: DashMap<QueryDefinitionKey, QueryDefinitionInput>,
    fragment_inputs: DashMap<FragmentDefinitionKey, FragmentDefinitionInput>,
    file_queries: DashMap<FileId, Vec<QueryDefinitionKey>>,
    file_fragments: DashMap<FileId, Vec<FragmentDefinitionKey>>,
    context_inputs: DashMap<String, ContextSourcesInput>,
}

impl Default for CompilerDb {
    fn default() -> Self {
        let db = Self::new(
            DashMap::new(),
            DashMap::new(),
            DashMap::new(),
            DashMap::new(),
            DashMap::new(),
            DashMap::new(),
        );
        db.set_catalog(Catalog::hardcoded())
            .expect("hardcoded catalog should be representable by Picante");
        db.set_lint_options(LintOptions::default())
            .expect("default lint options should be representable by Picante");
        db
    }
}

impl CompilerDb {
    pub(crate) fn set_catalog(&self, catalog: Catalog) -> PicanteResult<()> {
        CatalogInput::set(self, catalog)
    }

    pub(crate) fn set_lint_options(&self, options: LintOptions) -> PicanteResult<()> {
        LintOptionsInput::set(self, options)
    }

    pub(crate) fn set_source_rope(
        &self,
        file: FileId,
        revision: RevisionId,
        rope: Rope,
    ) -> PicanteResult<()> {
        let text = Arc::<str>::from(rope.to_string());
        let source = SourceInput::new(self, file, revision, text)?;
        self.source_inputs.insert(file, source);
        self.update_definition_inputs(file, source)?;
        Ok(())
    }

    pub(crate) fn set_indexed_source_rope(&self, file: FileId, rope: Rope) -> PicanteResult<()> {
        self.source_inputs.remove(&file);
        let text = Arc::<str>::from(rope.to_string());
        let parsed = parse_source(SourceSnapshot::from_arc(text));
        let extracted = extract_definitions(&parsed.source_file);
        self.update_definition_records(file, extracted.definitions)
    }

    pub(crate) fn remove_source(&self, file: FileId) {
        self.source_inputs.remove(&file);
        self.clear_definition_inputs(file);
    }

    fn source_input(&self, file: FileId) -> Option<SourceInput> {
        self.source_inputs.get(&file).map(|source| *source)
    }

    pub(crate) async fn analysis(&self, file: FileId) -> Option<AnalysisResult> {
        let source = self.source_input(file)?;
        let parsed = parse_file(self, source).await.ok()?;
        let lower = lower_file_query(self, source).await.ok()?;
        let check = self.definition_check(file).await.ok()?;
        let lint = self.definition_lint(file).await.ok()?;
        let plan = self.definition_plan(file).await.ok()?;
        let text = source.text(self).ok()?;
        Some(AnalysisResult {
            parse: ParseResult {
                source: SourceSnapshot::from_arc(text),
                tree: parsed.tree,
                source_file: parsed.source_file,
                diagnostics: parsed.diagnostics,
            },
            lower,
            check,
            lint,
            plan,
        })
    }

    pub(crate) async fn diagnostics(&self, file: FileId) -> PicanteResult<Vec<Diagnostic>> {
        let Some(source) = self.source_input(file) else {
            return Ok(Vec::new());
        };
        let parse = parse_file(self, source).await?;
        let lower = lower_file_query(self, source).await?;
        let check = self.definition_check(file).await?;
        let lint = self.definition_lint(file).await?;
        let plan = self.definition_plan(file).await?;
        Ok(collect_diagnostics_parts(&[
            &parse, &lower, &check, &lint, &plan,
        ]))
    }

    pub(crate) fn set_context_files(
        &self,
        context: String,
        files: Vec<FileId>,
    ) -> PicanteResult<()> {
        let mut seen = HashSet::new();
        let sources = files
            .into_iter()
            .filter(|file| seen.insert(*file))
            .filter_map(|file| {
                self.source_input(file)
                    .map(|source| ContextSource { file, source })
            })
            .collect();
        let input = ContextSourcesInput::new(self, context.clone(), sources)?;
        self.context_inputs.insert(context, input);
        Ok(())
    }

    pub(crate) async fn diagnostics_in_context(
        &self,
        context: &str,
        file: FileId,
    ) -> PicanteResult<Vec<Diagnostic>> {
        let Some(context) = self.context_inputs.get(context).map(|input| *input) else {
            return Ok(Vec::new());
        };
        let Some(source) = self.source_input(file) else {
            return Ok(Vec::new());
        };
        diagnostics_for_file_in_context(self, context, file, source).await
    }

    pub(crate) async fn context_definitions(
        &self,
        context: &str,
    ) -> PicanteResult<ContextDefinitions> {
        let Some(context) = self.context_inputs.get(context).map(|input| *input) else {
            return Ok(ContextDefinitions { files: Vec::new() });
        };
        context_definitions_query(self, context).await
    }

    pub(crate) async fn completion_scope_in_context(
        &self,
        context: &str,
        excluding: FileId,
    ) -> PicanteResult<CompletionScope> {
        let definitions = self.context_definitions(context).await?;
        let mut fragments = Vec::new();
        let mut seen = HashSet::<String>::new();
        for file in definitions.files {
            if file.file == excluding {
                continue;
            }
            for definition in file.definitions {
                if let DefinitionRecord::Fragment(fragment) = definition
                    && seen.insert(fragment.key.name.clone())
                {
                    fragments.push(fragment);
                }
            }
        }
        fragments.sort_by(|left, right| left.key.name.cmp(&right.key.name));
        Ok(CompletionScope { fragments })
    }

    async fn definition_check(&self, file: FileId) -> PicanteResult<CheckedFile> {
        let mut diagnostics = Vec::new();
        if let Some(queries) = self.file_queries.get(&file) {
            for key in queries.iter() {
                if let Some(input) = self.query_inputs.get(key).map(|input| *input) {
                    diagnostics
                        .extend(check_query_definition_query(self, input).await?.diagnostics);
                }
            }
        }
        if let Some(fragments) = self.file_fragments.get(&file) {
            for name in fragments.iter() {
                if let Some(input) = self.fragment_inputs.get(name).map(|input| *input) {
                    diagnostics.extend(
                        check_fragment_definition_query(self, input)
                            .await?
                            .diagnostics,
                    );
                }
            }
        }
        diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
        Ok(CheckedFile {
            errors: Vec::new(),
            diagnostics,
        })
    }

    async fn definition_lint(&self, file: FileId) -> PicanteResult<LintedFile> {
        let mut diagnostics = Vec::new();
        if let Some(queries) = self.file_queries.get(&file) {
            for key in queries.iter() {
                if let Some(input) = self.query_inputs.get(key).map(|input| *input) {
                    diagnostics.extend(lint_query_definition_query(self, input).await?.diagnostics);
                }
            }
        }
        if let Some(fragments) = self.file_fragments.get(&file) {
            for name in fragments.iter() {
                if let Some(input) = self.fragment_inputs.get(name).map(|input| *input) {
                    diagnostics.extend(
                        lint_fragment_definition_query(self, input)
                            .await?
                            .diagnostics,
                    );
                }
            }
        }
        diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
        Ok(LintedFile { diagnostics })
    }

    async fn definition_plan(&self, file: FileId) -> PicanteResult<PlannedFile> {
        let mut queries = Vec::new();
        let mut diagnostics = Vec::new();
        if let Some(query_keys) = self.file_queries.get(&file) {
            for key in query_keys.iter() {
                if let Some(input) = self.query_inputs.get(key).map(|input| *input) {
                    let plan = plan_query_definition_query(self, input).await?;
                    queries.extend(plan.queries);
                    diagnostics.extend(plan.diagnostics);
                }
            }
        }
        diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
        Ok(PlannedFile {
            queries,
            diagnostics,
        })
    }

    pub(crate) async fn formatted_text(&self, file: FileId) -> PicanteResult<Option<String>> {
        let Some(source) = self.source_input(file) else {
            return Ok(None);
        };
        formatted_text_for_file(self, source).await
    }

    pub(crate) async fn completion_scope(&self, file: FileId) -> PicanteResult<CompletionScope> {
        let fragments = self.fragment_inputs_for_completion(file);
        let input = CompletionScopeInput::new(self, file, fragments)?;
        completion_scope_query(self, input).await
    }

    pub(crate) fn fragment_definition(&self, name: &str) -> Option<FragmentDefinitionLocation> {
        let input = self.fragment_inputs_by_name(name).into_iter().next()?;
        let record = input.record(self).ok()??;
        let file = self.indexed_files_for_fragment(name).into_iter().next()?;
        Some(FragmentDefinitionLocation {
            file,
            range: record.name_range,
        })
    }

    pub(crate) fn catalog(&self) -> Catalog {
        CatalogInput::catalog(self)
            .ok()
            .flatten()
            .unwrap_or_else(Catalog::hardcoded)
    }

    pub(crate) fn lint_options(&self) -> LintOptions {
        LintOptionsInput::options(self)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn update_definition_inputs(&self, file: FileId, source: SourceInput) -> PicanteResult<()> {
        let text = source.text(self)?;
        let parsed = parse_source(SourceSnapshot::from_arc(text));
        let extracted = extract_definitions(&parsed.source_file);
        self.update_definition_records(file, extracted.definitions)
    }

    fn update_definition_records(
        &self,
        file: FileId,
        definitions: Vec<dsql_core::DefinitionRecord>,
    ) -> PicanteResult<()> {
        let old_queries = self
            .file_queries
            .get(&file)
            .map(|queries| queries.clone())
            .unwrap_or_default();
        let old_fragments = self
            .file_fragments
            .get(&file)
            .map(|fragments| fragments.clone())
            .unwrap_or_default();
        let mut next_queries = Vec::new();
        let mut next_fragments = Vec::new();
        let mut seen_queries = HashSet::new();
        let mut seen_fragments = HashSet::new();
        let mut fragment_ordinal = 0;
        for definition in definitions {
            match definition {
                dsql_core::DefinitionRecord::Query(record) => {
                    let key = QueryDefinitionKey {
                        file,
                        ordinal: record.key.ordinal,
                    };
                    seen_queries.insert(key);
                    next_queries.push(key);
                    self.set_query_record(key, Some(record))?;
                }
                dsql_core::DefinitionRecord::Fragment(record) => {
                    let key = FragmentDefinitionKey {
                        file,
                        ordinal: fragment_ordinal,
                        name: record.key.name.clone(),
                    };
                    fragment_ordinal += 1;
                    seen_fragments.insert(key.clone());
                    next_fragments.push(key.clone());
                    self.set_fragment_record(key, Some(record))?;
                }
            }
        }
        for key in old_queries {
            if !seen_queries.contains(&key) {
                self.set_query_record(key, None)?;
            }
        }
        for key in old_fragments {
            if !seen_fragments.contains(&key) {
                self.set_fragment_record(key, None)?;
            }
        }
        self.file_queries.insert(file, next_queries);
        self.file_fragments.insert(file, next_fragments);
        self.refresh_definition_dependencies()?;
        Ok(())
    }

    fn clear_definition_inputs(&self, file: FileId) {
        if let Some((_, queries)) = self.file_queries.remove(&file) {
            for key in queries {
                let _ = self.set_query_record(key, None);
            }
        }
        if let Some((_, fragments)) = self.file_fragments.remove(&file) {
            for key in fragments {
                let _ = self.set_fragment_record(key, None);
            }
        }
    }

    fn fragment_inputs_for_completion(&self, excluding: FileId) -> Vec<FragmentDefinitionInput> {
        let mut names = HashSet::new();
        let mut inputs = Vec::new();
        for entry in self.file_fragments.iter() {
            if *entry.key() == excluding {
                continue;
            }
            for key in entry.value() {
                if names.insert(key.name.clone())
                    && let Some(input) = self.fragment_inputs.get(key).map(|input| *input)
                {
                    inputs.push(input);
                }
            }
        }
        inputs.sort_by_key(|input| input.name(self).ok());
        inputs
    }

    fn indexed_files_for_fragment(&self, name: &str) -> Vec<FileId> {
        let mut files = self
            .file_fragments
            .iter()
            .filter_map(|entry| {
                entry
                    .value()
                    .iter()
                    .any(|fragment| fragment.name == name)
                    .then_some(*entry.key())
            })
            .collect::<Vec<_>>();
        files.sort_by_key(|file| file.0);
        files
    }

    fn set_query_record(
        &self,
        key: QueryDefinitionKey,
        record: Option<QueryRecord>,
    ) -> PicanteResult<()> {
        let fragments = record
            .as_ref()
            .map(|record| self.fragment_inputs_for_record(record))
            .transpose()?
            .unwrap_or_default();
        let input = QueryDefinitionInput::new(self, key, record.clone(), fragments)?;
        self.query_inputs.insert(key, input);
        Ok(())
    }

    fn set_fragment_record(
        &self,
        key: FragmentDefinitionKey,
        record: Option<FragmentRecord>,
    ) -> PicanteResult<()> {
        let fragments = record
            .as_ref()
            .map(|record| self.fragment_inputs_for_record(record))
            .transpose()?
            .unwrap_or_default();
        let input = FragmentDefinitionInput::new(
            self,
            key.clone(),
            key.name.clone(),
            record.clone(),
            fragments,
        )?;
        self.fragment_inputs.insert(key, input);
        Ok(())
    }

    fn refresh_definition_dependencies(&self) -> PicanteResult<()> {
        let query_records = self
            .query_inputs
            .iter()
            .filter_map(|entry| {
                entry
                    .record(self)
                    .ok()
                    .map(|record| ((*entry.key()), record))
            })
            .collect::<Vec<_>>();
        let fragment_records = self
            .fragment_inputs
            .iter()
            .filter_map(|entry| {
                if entry.key().file == FileId(u32::MAX) {
                    return None;
                }
                entry
                    .record(self)
                    .ok()
                    .map(|record| (entry.key().clone(), record))
            })
            .collect::<Vec<_>>();

        for (key, record) in query_records {
            self.set_query_record(key, record)?;
        }
        for (key, record) in fragment_records {
            self.set_fragment_record(key, record)?;
        }
        Ok(())
    }

    fn fragment_inputs_for_record(
        &self,
        record: &impl FragmentSpreadSource,
    ) -> PicanteResult<Vec<FragmentDefinitionInput>> {
        let mut inputs = Vec::new();
        for name in record.fragment_spread_names() {
            let found = self.fragment_inputs_by_name(&name);
            if found.is_empty() {
                inputs.push(self.missing_fragment_input(name)?);
            } else {
                inputs.extend(found);
            }
        }
        Ok(inputs)
    }

    fn fragment_inputs_by_name(&self, name: &str) -> Vec<FragmentDefinitionInput> {
        let mut inputs = self
            .fragment_inputs
            .iter()
            .filter_map(|entry| (entry.key().name == name).then_some((*entry.key()).clone()))
            .collect::<Vec<_>>();
        inputs.sort_by_key(|key| (key.file.0, key.ordinal));
        inputs
            .into_iter()
            .filter_map(|key| self.fragment_inputs.get(&key).map(|input| *input))
            .collect()
    }

    fn missing_fragment_input(&self, name: String) -> PicanteResult<FragmentDefinitionInput> {
        let key = FragmentDefinitionKey {
            file: FileId(u32::MAX),
            ordinal: u32::MAX,
            name: name.clone(),
        };
        if let Some(input) = self.fragment_inputs.get(&key).map(|input| *input) {
            return Ok(input);
        }
        let input = FragmentDefinitionInput::new(self, key.clone(), name, None, Vec::new())?;
        self.fragment_inputs.insert(key, input);
        Ok(input)
    }
}

trait FragmentSpreadSource {
    fn fragment_spread_names(&self) -> Vec<String>;
}

impl FragmentSpreadSource for QueryRecord {
    fn fragment_spread_names(&self) -> Vec<String> {
        self.fragment_spreads
            .iter()
            .map(|spread| spread.name.clone())
            .collect()
    }
}

impl FragmentSpreadSource for FragmentRecord {
    fn fragment_spread_names(&self) -> Vec<String> {
        self.fragment_spreads
            .iter()
            .map(|spread| spread.name.clone())
            .collect()
    }
}
