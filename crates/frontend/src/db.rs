use crate::analysis::collect_diagnostics_parts;
use crate::completion::CompletionScope;
use crate::document::{RevisionId, SourceUnitId};
use dashmap::DashMap;
use dsql_core::{
    Catalog, CheckDiagnostic, CheckDiagnosticKind, CheckedFile, CompilerDiagnostic,
    CompilerDiagnosticSource, DefinitionRecord, Diagnostic, ExtractedFile, FragmentMap,
    FragmentRecord, Interner, LintOptions, LintedFile, LoweredFile, ParseResult, PlannedFile,
    SourceFile, SourceSnapshot, SyntaxTree, check_file_with_catalog, check_fragment_definition,
    check_query_definition, extract_definitions, format_file, lint_file_with_options,
    lint_fragment_definition_with_options, lint_query_definition_with_options, lower_file,
    parse_source, plan_file_with_catalog, plan_query_definition,
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
    pub unit_id: SourceUnitId,
    pub revision: RevisionId,
    pub text: Arc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
pub struct ContextSource {
    pub unit_id: SourceUnitId,
    pub source: SourceInput,
}

#[derive(Clone, Debug, Facet)]
pub struct ContextDefinitionFile {
    pub unit_id: SourceUnitId,
    pub definitions: Vec<DefinitionRecord>,
}

#[derive(Clone, Debug, Facet)]
pub struct ContextDefinitions {
    pub unit_ids: Vec<ContextDefinitionFile>,
}

#[derive(Clone, Debug, Facet)]
pub struct ScopedProgram {
    pub env_id: String,
    pub units: Vec<ContextDefinitionFile>,
    pub fragments: Vec<FragmentRecord>,
    pub diagnostics: Vec<ScopedDiagnostic>,
}

#[derive(Clone, Debug, Facet)]
pub struct ScopedDiagnostic {
    pub unit_id: SourceUnitId,
    pub diagnostic: Diagnostic,
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

#[picante::tracked]
pub async fn parse_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<Arc<ParsedFile>> {
    let text = source.text(db)?;
    let parse = parse_source(SourceSnapshot::from_arc(text));
    Ok(Arc::new(ParsedFile {
        tree: parse.tree,
        source_file: parse.source_file,
        diagnostics: parse.diagnostics,
    }))
}

#[picante::tracked]
pub async fn lower_file_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<Arc<LoweredFile>> {
    let parse = parse_file(db, source).await?;
    let mut interner = Interner::default();
    Ok(Arc::new(lower_file(&parse.source_file, &mut interner)))
}

#[picante::tracked]
pub async fn extract_definitions_for_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<Arc<ExtractedFile>> {
    let parse = parse_file(db, source).await?;
    Ok(Arc::new(extract_definitions(&parse.source_file)))
}

#[picante::tracked]
pub async fn context_definitions_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    context: ContextSourcesInput,
) -> PicanteResult<ContextDefinitions> {
    let mut unit_ids = Vec::new();
    for source in context.sources(db)? {
        let extracted = extract_definitions_for_file(db, source.source).await?;
        unit_ids.push(ContextDefinitionFile {
            unit_id: source.unit_id,
            definitions: extracted.definitions.clone(),
        });
    }
    Ok(ContextDefinitions { unit_ids })
}

#[picante::tracked]
pub async fn scoped_program_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    context: ContextSourcesInput,
) -> PicanteResult<Arc<ScopedProgram>> {
    let env_id = context.context(db)?.to_string();
    let definitions = context_definitions_query(db, context).await?;
    let fragments = context_fragments(&definitions);
    let diagnostics = duplicate_fragment_diagnostics(&definitions);
    Ok(Arc::new(ScopedProgram {
        env_id,
        units: definitions.unit_ids,
        fragments,
        diagnostics,
    }))
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
pub async fn diagnostics_for_unit_in_scope<DB: CompilerDatabaseTrait>(
    db: &DB,
    context: ContextSourcesInput,
    unit_id: SourceUnitId,
    source: SourceInput,
) -> PicanteResult<Vec<Diagnostic>> {
    let parse = parse_file(db, source).await?;
    let lower = lower_file_query(db, source).await?;
    let scoped = scoped_program_query(db, context).await?;
    let catalog = CatalogInput::catalog(db)?.unwrap_or_else(Catalog::hardcoded);
    let options = LintOptionsInput::options(db)?.unwrap_or_default();
    let check = scoped_check_unit(&scoped, unit_id, &catalog);
    let lint = scoped_lint_unit(&scoped, unit_id, &catalog, options);
    let plan = scoped_plan_unit(&scoped, unit_id, &catalog);
    let mut diagnostics =
        collect_diagnostics_parts(&[parse.as_ref(), lower.as_ref(), &check, &lint, &plan]);
    diagnostics.extend(
        scoped
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.unit_id == unit_id)
            .map(|diagnostic| diagnostic.diagnostic.clone()),
    );
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    Ok(diagnostics)
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
        tree: parsed.tree.clone(),
        source_file: parsed.source_file.clone(),
        diagnostics: parsed.diagnostics.clone(),
    };
    let formatted = format_file(&parse);
    if formatted.diagnostics.is_empty() {
        Ok(Some(formatted.text))
    } else {
        Ok(None)
    }
}

fn context_fragments(definitions: &ContextDefinitions) -> Vec<FragmentRecord> {
    let mut fragments = Vec::new();
    for unit_id in &definitions.unit_ids {
        for definition in &unit_id.definitions {
            if let DefinitionRecord::Fragment(fragment) = definition {
                fragments.push(fragment.clone());
            }
        }
    }
    fragments
}

fn scoped_fragment_map(scoped: &ScopedProgram) -> FragmentMap {
    let mut fragments = FragmentMap::default();
    for fragment in &scoped.fragments {
        fragments.insert(fragment.clone());
    }
    fragments
}

fn scoped_check_unit(
    scoped: &ScopedProgram,
    unit_id: SourceUnitId,
    catalog: &Catalog,
) -> CheckedFile {
    let resolver = scoped_fragment_map(scoped);
    let mut diagnostics = Vec::new();
    for definition in definitions_for_unit(scoped, unit_id) {
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

fn scoped_lint_unit(
    scoped: &ScopedProgram,
    unit_id: SourceUnitId,
    catalog: &Catalog,
    options: LintOptions,
) -> LintedFile {
    let resolver = scoped_fragment_map(scoped);
    let mut diagnostics = Vec::new();
    for definition in definitions_for_unit(scoped, unit_id) {
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

fn scoped_plan_unit(
    scoped: &ScopedProgram,
    unit_id: SourceUnitId,
    catalog: &Catalog,
) -> PlannedFile {
    let resolver = scoped_fragment_map(scoped);
    let mut queries = Vec::new();
    let mut diagnostics = Vec::new();
    for definition in definitions_for_unit(scoped, unit_id) {
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

fn definitions_for_unit(
    scoped: &ScopedProgram,
    unit_id: SourceUnitId,
) -> impl Iterator<Item = &DefinitionRecord> {
    scoped
        .units
        .iter()
        .filter(move |definition_file| definition_file.unit_id == unit_id)
        .flat_map(|definition_file| definition_file.definitions.iter())
}

fn duplicate_fragment_diagnostics(definitions: &ContextDefinitions) -> Vec<ScopedDiagnostic> {
    let mut fragments = Vec::<(String, SourceUnitId, FragmentRecord)>::new();
    for definition_file in &definitions.unit_ids {
        for definition in &definition_file.definitions {
            if let DefinitionRecord::Fragment(fragment) = definition {
                fragments.push((
                    fragment.key.name.clone(),
                    definition_file.unit_id,
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
        diagnostics.push(ScopedDiagnostic {
            unit_id: fragment_file,
            diagnostic: CheckDiagnostic {
                range: fragment.name_range,
                kind: CheckDiagnosticKind::DuplicateFragment { name },
            }
            .to_diagnostic(),
        });
    }
    diagnostics.sort_by_key(|diagnostic| {
        (
            diagnostic.unit_id.0,
            diagnostic.diagnostic.range.start,
            diagnostic.diagnostic.range.end,
        )
    });
    diagnostics
}

#[picante::db(
    inputs(SourceInput, CatalogInput, LintOptionsInput, ContextSourcesInput),
    tracked(
        parse_file,
        lower_file_query,
        extract_definitions_for_file,
        context_definitions_query,
        scoped_program_query,
        check_file_query,
        plan_file_query,
        lint_file_query,
        diagnostics_for_unit_in_scope,
        formatted_text_for_file
    ),
    db_trait(CompilerDatabaseTrait)
)]
pub struct CompilerDb {
    source_inputs: DashMap<SourceUnitId, SourceInput>,
    context_inputs: DashMap<String, ContextSourcesInput>,
}

impl Default for CompilerDb {
    fn default() -> Self {
        let db = Self::new(DashMap::new(), DashMap::new());
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
        unit_id: SourceUnitId,
        revision: RevisionId,
        rope: Rope,
    ) -> PicanteResult<()> {
        let text = Arc::<str>::from(rope.to_string());
        let source = SourceInput::new(self, unit_id, revision, text)?;
        self.source_inputs.insert(unit_id, source);
        Ok(())
    }

    pub(crate) fn remove_source(&self, unit_id: SourceUnitId) {
        self.source_inputs.remove(&unit_id);
    }

    fn source_input(&self, unit_id: SourceUnitId) -> Option<SourceInput> {
        self.source_inputs.get(&unit_id).map(|source| *source)
    }

    pub(crate) async fn analysis_in_scope(
        &self,
        context: &str,
        unit_id: SourceUnitId,
    ) -> Option<AnalysisResult> {
        let context = self.context_inputs.get(context).map(|input| *input)?;
        let source = self.source_input(unit_id)?;
        let parsed = parse_file(self, source).await.ok()?;
        let lower = lower_file_query(self, source).await.ok()?;
        let scoped = scoped_program_query(self, context).await.ok()?;
        let catalog = self.catalog();
        let options = self.lint_options();
        let check = scoped_check_unit(&scoped, unit_id, &catalog);
        let lint = scoped_lint_unit(&scoped, unit_id, &catalog, options);
        let plan = scoped_plan_unit(&scoped, unit_id, &catalog);
        let text = source.text(self).ok()?;
        Some(AnalysisResult {
            parse: ParseResult {
                source: SourceSnapshot::from_arc(text),
                tree: parsed.tree.clone(),
                source_file: parsed.source_file.clone(),
                diagnostics: parsed.diagnostics.clone(),
            },
            lower: lower.as_ref().clone(),
            check,
            lint,
            plan,
        })
    }

    pub(crate) fn set_context_files(
        &self,
        context: String,
        unit_ids: Vec<SourceUnitId>,
    ) -> PicanteResult<()> {
        let mut seen = HashSet::new();
        let sources = unit_ids
            .into_iter()
            .filter(|unit_id| seen.insert(*unit_id))
            .filter_map(|unit_id| {
                self.source_input(unit_id)
                    .map(|source| ContextSource { unit_id, source })
            })
            .collect();
        let input = ContextSourcesInput::new(self, context.clone(), sources)?;
        self.context_inputs.insert(context, input);
        Ok(())
    }

    pub(crate) async fn diagnostics_in_scope(
        &self,
        context: &str,
        unit_id: SourceUnitId,
    ) -> PicanteResult<Vec<Diagnostic>> {
        let Some(context) = self.context_inputs.get(context).map(|input| *input) else {
            return Ok(Vec::new());
        };
        let Some(source) = self.source_input(unit_id) else {
            return Ok(Vec::new());
        };
        diagnostics_for_unit_in_scope(self, context, unit_id, source).await
    }

    pub(crate) async fn scoped_program(&self, context: &str) -> PicanteResult<Arc<ScopedProgram>> {
        let Some(context) = self.context_inputs.get(context).map(|input| *input) else {
            return Ok(Arc::new(ScopedProgram {
                env_id: context.to_string(),
                units: Vec::new(),
                fragments: Vec::new(),
                diagnostics: Vec::new(),
            }));
        };
        scoped_program_query(self, context).await
    }

    pub(crate) async fn completion_scope_in_context(
        &self,
        context: &str,
        excluding: SourceUnitId,
    ) -> PicanteResult<CompletionScope> {
        let scoped = self.scoped_program(context).await?;
        let mut fragments = Vec::new();
        let mut seen = HashSet::<String>::new();
        for unit in &scoped.units {
            if unit.unit_id == excluding {
                continue;
            }
            for definition in &unit.definitions {
                if let DefinitionRecord::Fragment(fragment) = definition
                    && seen.insert(fragment.key.name.clone())
                {
                    fragments.push(fragment.clone());
                }
            }
        }
        fragments.sort_by(|left, right| left.key.name.cmp(&right.key.name));
        Ok(CompletionScope { fragments })
    }

    pub(crate) async fn formatted_text(
        &self,
        unit_id: SourceUnitId,
    ) -> PicanteResult<Option<String>> {
        let Some(source) = self.source_input(unit_id) else {
            return Ok(None);
        };
        formatted_text_for_file(self, source).await
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
}
