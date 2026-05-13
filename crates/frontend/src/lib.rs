use dashmap::DashMap;
use dsql_core::{
    Catalog, CheckedFile, Definition, Diagnostic, FieldCheckResult, FormattedText, Interner,
    LintedFile, LoweredFile, ParseResult, PlannedFile, Selection, SourceFile, SourceSnapshot,
    SyntaxTree, TableId, TextRange, check_file_with_catalog, format_file, lint_file_with_catalog,
    lower_file, parse_source, plan_file_with_catalog,
};
use facet::Facet;
use picante::PicanteResult;
use ropey::{LineType, Rope};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
pub struct FileId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Facet)]
pub struct RevisionId(pub u64);

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

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    pub file: FileId,
    pub uri: String,
    pub version: i32,
    pub revision: RevisionId,
    pub rope: Rope,
}

#[derive(Clone, Debug)]
pub struct DocumentDiagnostics {
    pub snapshot: DocumentSnapshot,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct DocumentFormat {
    pub snapshot: DocumentSnapshot,
    pub formatted: FormattedText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEditRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Option<TextEditRange>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Table,
    Column,
    Relation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverInfo {
    pub label: String,
    pub detail: String,
    pub markdown: String,
}

#[derive(Clone, Debug)]
pub struct DocumentSemanticTokens {
    pub snapshot: DocumentSnapshot,
    pub tokens: Vec<SemanticTokenInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticTokenInfo {
    pub range: TextRange,
    pub kind: SemanticTokenKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticTokenKind {
    Schema,
    Table,
    Relation,
    Column,
    Fragment,
    Alias,
}

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

#[derive(Clone, Debug)]
struct DocumentState {
    file: FileId,
    uri: String,
    version: i32,
    revision: RevisionId,
    rope: Rope,
}

impl DocumentState {
    fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            file: self.file,
            uri: self.uri.clone(),
            version: self.version,
            revision: self.revision,
            rope: self.rope.clone(),
        }
    }
}

#[picante::input]
pub struct SourceInput {
    #[key]
    pub file: FileId,
    pub revision: RevisionId,
    pub text: Arc<str>,
}

#[picante::input]
pub struct CatalogInput {
    pub catalog: Catalog,
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
    Ok(lint_file_with_catalog(&parse.source_file, &catalog))
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
    Ok(collect_diagnostics_parts(
        &parse.diagnostics,
        &lower.diagnostics,
        &check.diagnostics,
        &lint.diagnostics,
    ))
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

#[picante::db(
    inputs(SourceInput, CatalogInput),
    tracked(
        parse_file,
        lower_file_query,
        check_file_query,
        plan_file_query,
        lint_file_query,
        diagnostics_for_file,
        formatted_text_for_file
    ),
    db_trait(CompilerDatabaseTrait)
)]
pub struct CompilerDb {
    source_inputs: DashMap<FileId, SourceInput>,
}

impl Default for CompilerDb {
    fn default() -> Self {
        let db = Self::new(DashMap::new());
        db.set_catalog(Catalog::hardcoded())
            .expect("hardcoded catalog should be representable by Picante");
        db
    }
}

impl CompilerDb {
    fn set_catalog(&self, catalog: Catalog) -> PicanteResult<()> {
        CatalogInput::set(self, catalog)
    }

    fn set_source_rope(&self, file: FileId, revision: RevisionId, rope: Rope) -> PicanteResult<()> {
        let text = Arc::<str>::from(rope.to_string());
        let source = SourceInput::new(self, file, revision, text)?;
        self.source_inputs.insert(file, source);
        Ok(())
    }

    fn remove_source(&self, file: FileId) {
        self.source_inputs.remove(&file);
    }

    fn source_input(&self, file: FileId) -> Option<SourceInput> {
        self.source_inputs.get(&file).map(|source| *source)
    }

    async fn analysis(&self, file: FileId) -> Option<AnalysisResult> {
        let source = self.source_input(file)?;
        let parsed = parse_file(self, source).await.ok()?;
        let lower = lower_file_query(self, source).await.ok()?;
        let check = check_file_query(self, source).await.ok()?;
        let lint = lint_file_query(self, source).await.ok()?;
        let plan = plan_file_query(self, source).await.ok()?;
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

    async fn diagnostics(&self, file: FileId) -> PicanteResult<Vec<Diagnostic>> {
        let Some(source) = self.source_input(file) else {
            return Ok(Vec::new());
        };
        diagnostics_for_file(self, source).await
    }

    async fn formatted_text(&self, file: FileId) -> PicanteResult<Option<String>> {
        let Some(source) = self.source_input(file) else {
            return Ok(None);
        };
        formatted_text_for_file(self, source).await
    }

    fn catalog(&self) -> Catalog {
        CatalogInput::catalog(self)
            .ok()
            .flatten()
            .unwrap_or_else(Catalog::hardcoded)
    }
}

#[derive(Clone)]
pub struct AnalysisHost {
    inner: Arc<AnalysisHostInner>,
}

struct AnalysisHostInner {
    db: CompilerDb,
    next_file: AtomicU32,
    documents: DashMap<String, DocumentState>,
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new()
    }
}

impl AnalysisHost {
    pub fn new() -> Self {
        Self::with_catalog_provider(HardcodedCatalogProvider)
    }

    pub fn with_catalog_provider(provider: impl CatalogProvider) -> Self {
        let db = CompilerDb::default();
        db.set_catalog(provider.load_catalog())
            .expect("catalog should be representable by Picante");
        Self {
            inner: Arc::new(AnalysisHostInner {
                db,
                next_file: AtomicU32::new(0),
                documents: DashMap::new(),
            }),
        }
    }

    pub fn set_catalog(&self, catalog: Catalog) {
        self.inner
            .db
            .set_catalog(catalog)
            .expect("catalog should be representable by Picante");
    }

    pub fn create_file(&self, source: SourceSnapshot) -> FileId {
        let id = self.alloc_file();
        self.inner
            .db
            .set_source_rope(id, RevisionId(0), source.into_rope())
            .expect("source input should be representable by Picante");
        id
    }

    pub fn set_file_source(&self, file: FileId, revision: RevisionId, source: SourceSnapshot) {
        self.inner
            .db
            .set_source_rope(file, revision, source.into_rope())
            .expect("source input should be representable by Picante");
    }

    pub async fn analyze(&self, file: FileId) -> Option<AnalysisResult> {
        self.inner.db.analysis(file).await
    }

    pub async fn diagnostics(&self, file: FileId) -> Option<Vec<Diagnostic>> {
        self.inner.db.diagnostics(file).await.ok()
    }

    pub async fn format(&self, file: FileId) -> Option<FormattedText> {
        let text = self.inner.db.formatted_text(file).await.ok()??;
        Some(FormattedText {
            text,
            confidence: dsql_core::FormatConfidence::Full,
            diagnostics: Vec::new(),
        })
    }

    pub async fn open_document(
        &self,
        uri: String,
        version: i32,
        text: String,
    ) -> DocumentDiagnostics {
        let file = match self.inner.documents.get(&uri) {
            Some(document) => document.file,
            None => self.alloc_file(),
        };
        let rope = Rope::from_str(&text);
        let revision = RevisionId(version.max(0) as u64);
        self.inner
            .db
            .set_source_rope(file, revision, rope.clone())
            .ok();
        let state = DocumentState {
            file,
            uri: uri.clone(),
            version,
            revision,
            rope,
        };
        let snapshot = state.snapshot();
        self.inner.documents.insert(uri.clone(), state);
        self.document_diagnostics(&uri)
            .await
            .unwrap_or_else(|| DocumentDiagnostics {
                snapshot,
                diagnostics: Vec::new(),
            })
    }

    pub async fn change_document(
        &self,
        uri: String,
        version: i32,
        edits: Vec<TextEdit>,
    ) -> Option<DocumentDiagnostics> {
        let snapshot = {
            let mut document = self.inner.documents.get_mut(&uri)?;
            apply_text_edits(&mut document.rope, edits);
            document.version = version;
            document.revision = RevisionId(version.max(0) as u64);
            document.snapshot()
        };
        self.inner
            .db
            .set_source_rope(snapshot.file, snapshot.revision, snapshot.rope.clone())
            .ok()?;
        self.document_diagnostics(&uri).await
    }

    pub async fn replace_document(
        &self,
        uri: String,
        version: i32,
        text: String,
    ) -> Option<DocumentDiagnostics> {
        let file = self.inner.documents.get(&uri)?.file;
        let rope = Rope::from_str(&text);
        let revision = RevisionId(version.max(0) as u64);
        self.inner
            .db
            .set_source_rope(file, revision, rope.clone())
            .ok()?;
        let state = DocumentState {
            file,
            uri: uri.clone(),
            version,
            revision,
            rope,
        };
        self.inner.documents.insert(uri.clone(), state);
        self.document_diagnostics(&uri).await
    }

    pub fn close_document(&self, uri: &str) -> Option<DocumentSnapshot> {
        let (_, state) = self.inner.documents.remove(uri)?;
        self.inner.db.remove_source(state.file);
        Some(state.snapshot())
    }

    pub async fn document_diagnostics(&self, uri: &str) -> Option<DocumentDiagnostics> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let diagnostics = self.inner.db.diagnostics(snapshot.file).await.ok()?;
        Some(DocumentDiagnostics {
            snapshot,
            diagnostics,
        })
    }

    pub async fn document_format(&self, uri: &str) -> Option<DocumentFormat> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let formatted = self.format(snapshot.file).await?;
        Some(DocumentFormat {
            snapshot,
            formatted,
        })
    }

    pub async fn completions(
        &self,
        uri: &str,
        position: TextPosition,
    ) -> Option<Vec<CompletionItem>> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let analysis = self.analyze(snapshot.file).await?;
        let catalog = self.inner.db.catalog();
        Some(completions_at(&analysis.parse.source_file, &catalog, byte))
    }

    pub async fn hover(&self, uri: &str, position: TextPosition) -> Option<HoverInfo> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let analysis = self.analyze(snapshot.file).await?;
        let catalog = self.inner.db.catalog();
        hover_at(&analysis.parse.source_file, &catalog, byte)
    }

    pub async fn semantic_tokens(&self, uri: &str) -> Option<DocumentSemanticTokens> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let analysis = self.analyze(snapshot.file).await?;
        let catalog = self.inner.db.catalog();
        Some(DocumentSemanticTokens {
            snapshot,
            tokens: semantic_tokens_at(&analysis.parse, &catalog),
        })
    }

    fn alloc_file(&self) -> FileId {
        FileId(self.inner.next_file.fetch_add(1, Ordering::Relaxed))
    }
}

fn completions_at(source_file: &SourceFile, catalog: &Catalog, byte: usize) -> Vec<CompletionItem> {
    match selection_context(source_file, catalog, byte) {
        CompletionContext::Root => catalog
            .tables
            .iter()
            .map(|table| CompletionItem {
                label: completion_table_ref(&table.schema, &table.name),
                kind: CompletionKind::Table,
                detail: Some(format!("table {}.{}", table.schema, table.name)),
            })
            .collect(),
        CompletionContext::Table(table) => field_completions(catalog, table),
    }
}

fn hover_at(source_file: &SourceFile, catalog: &Catalog, byte: usize) -> Option<HoverInfo> {
    for definition in source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    if !contains(selection.name.range, byte) && !contains(selection.range, byte) {
                        continue;
                    }
                    let table = catalog.table_ref(&selection.name.text)?;
                    if contains(selection.name.range, byte) {
                        return Some(HoverInfo {
                            label: selection.name.text.clone(),
                            detail: format!("table {}.{}", table.schema, table.name),
                            markdown: table_hover_markdown(table),
                        });
                    }
                    if let Some(info) =
                        hover_in_selections(catalog, table.id, &selection.selections, byte)
                    {
                        return Some(info);
                    }
                }
            }
            Definition::Fragment(fragment) => {
                if !contains(fragment.range, byte) {
                    continue;
                }
                let Some(on) = &fragment.on else {
                    continue;
                };
                let Some(table) = catalog.table_ref(&on.text) else {
                    continue;
                };
                if contains(on.range, byte) {
                    return Some(HoverInfo {
                        label: on.text.clone(),
                        detail: format!("table {}.{}", table.schema, table.name),
                        markdown: table_hover_markdown(table),
                    });
                }
                if let Some(info) =
                    hover_in_selections(catalog, table.id, &fragment.selections, byte)
                {
                    return Some(info);
                }
            }
        }
    }
    None
}

fn semantic_tokens_at(parse: &ParseResult, catalog: &Catalog) -> Vec<SemanticTokenInfo> {
    let mut tokens = Vec::new();
    for definition in parse.source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                if let Some(name) = &query.name {
                    tokens.push(SemanticTokenInfo {
                        range: name.range,
                        kind: SemanticTokenKind::Fragment,
                    });
                }
                for selection in &query.selections {
                    add_table_ref_tokens(&mut tokens, &parse.source, selection.name.range);
                    if let Some(table) = catalog.table_ref(&selection.name.text) {
                        add_selection_tokens(
                            &mut tokens,
                            &parse.source,
                            catalog,
                            table.id,
                            &selection.selections,
                        );
                    }
                }
            }
            Definition::Fragment(fragment) => {
                if let Some(name) = &fragment.name {
                    tokens.push(SemanticTokenInfo {
                        range: name.range,
                        kind: SemanticTokenKind::Fragment,
                    });
                }
                if let Some(on) = &fragment.on {
                    add_table_ref_tokens(&mut tokens, &parse.source, on.range);
                    if let Some(table) = catalog.table_ref(&on.text) {
                        add_selection_tokens(
                            &mut tokens,
                            &parse.source,
                            catalog,
                            table.id,
                            &fragment.selections,
                        );
                    }
                }
            }
        }
    }
    tokens.sort_by_key(|token| (token.range.start, token.range.end));
    tokens
}

fn add_selection_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
) {
    for selection in selections {
        if let Some(alias) = &selection.alias {
            tokens.push(SemanticTokenInfo {
                range: alias.range,
                kind: SemanticTokenKind::Alias,
            });
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(_) => {
                tokens.push(SemanticTokenInfo {
                    range: selection.name.range,
                    kind: SemanticTokenKind::Column,
                });
            }
            FieldCheckResult::Relation(relation) => {
                add_relation_ref_tokens(tokens, source, selection.name.range);
                add_selection_tokens(
                    tokens,
                    source,
                    catalog,
                    relation.table.id,
                    &selection.selections,
                );
            }
            FieldCheckResult::NotFound | FieldCheckResult::AmbiguousRelation { .. } => {}
        }
    }
}

fn add_table_ref_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    range: TextRange,
) {
    add_qualified_ref_tokens(tokens, source, range, SemanticTokenKind::Table);
}

fn add_relation_ref_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    range: TextRange,
) {
    add_qualified_ref_tokens(tokens, source, range, SemanticTokenKind::Relation);
}

fn add_qualified_ref_tokens(
    tokens: &mut Vec<SemanticTokenInfo>,
    source: &SourceSnapshot,
    range: TextRange,
    tail_kind: SemanticTokenKind,
) {
    let text = source.text(range);
    if let Some(dot) = text.find('.') {
        tokens.push(SemanticTokenInfo {
            range: TextRange {
                start: range.start,
                end: range.start + dot as u32,
            },
            kind: SemanticTokenKind::Schema,
        });
        tokens.push(SemanticTokenInfo {
            range: TextRange {
                start: range.start + dot as u32 + 1,
                end: range.end,
            },
            kind: tail_kind,
        });
    } else {
        tokens.push(SemanticTokenInfo {
            range,
            kind: tail_kind,
        });
    }
}

fn hover_in_selections(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    byte: usize,
) -> Option<HoverInfo> {
    for selection in selections {
        if !contains(selection.name.range, byte) && !contains(selection.range, byte) {
            continue;
        }
        match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Column(column) if contains(selection.name.range, byte) => {
                return Some(HoverInfo {
                    label: selection.name.text.clone(),
                    detail: format!("column: {}", column.data_type.as_str()),
                    markdown: column_hover_markdown(catalog, column),
                });
            }
            FieldCheckResult::Relation(related) => {
                if contains(selection.name.range, byte) {
                    return Some(HoverInfo {
                        label: selection.name.text.clone(),
                        detail: format!(
                            "relation: {}.{}",
                            related.table.schema, related.table.name
                        ),
                        markdown: relation_hover_markdown(catalog, &related),
                    });
                }
                if let Some(info) =
                    hover_in_selections(catalog, related.table.id, &selection.selections, byte)
                {
                    return Some(info);
                }
            }
            FieldCheckResult::NotFound
            | FieldCheckResult::Column(_)
            | FieldCheckResult::AmbiguousRelation { .. } => {}
        }
    }
    None
}

fn table_hover_markdown(table: &dsql_core::Table) -> String {
    format!(
        "### Table `{}`\n\n- Schema: `{}`\n- Columns: {}\n- Primary key columns: {}\n- Outgoing foreign keys: {}\n- Incoming foreign keys: {}",
        table.name,
        table.schema,
        table.columns.len(),
        table.primary_key.len(),
        table.foreign_keys_from.len(),
        table.foreign_keys_to.len()
    )
}

fn column_hover_markdown(catalog: &Catalog, column: &dsql_core::Column) -> String {
    let table = catalog.table_by_id(column.table);
    let primary_key = table.is_some_and(|table| table.primary_key.contains(&column.id));
    let table_name = table.map_or("<unknown>", |table| table.name.as_str());
    let schema_name = table.map_or("<unknown>", |table| table.schema.as_str());
    format!(
        "### Column `{}`\n\n- Table: `{}.{}`\n- Type: `{}`\n- Nullable: {}\n- Primary key: {}\n- Unique: {}\n- Indexed: {}",
        column.name,
        schema_name,
        table_name,
        column.data_type.as_str(),
        yes_no(!column.not_null),
        yes_no(primary_key),
        yes_no(column.is_unique),
        yes_no(column.is_indexed)
    )
}

fn relation_hover_markdown(catalog: &Catalog, relation: &dsql_core::RelationField<'_>) -> String {
    let from_table = catalog.table_by_id(relation.foreign_key.from_table);
    let to_table = catalog.table_by_id(relation.foreign_key.to_table);
    let from_columns = relation
        .foreign_key
        .from_columns
        .iter()
        .filter_map(|id| catalog.column_by_id(*id))
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let to_columns = relation
        .foreign_key
        .to_columns
        .iter()
        .filter_map(|id| catalog.column_by_id(*id))
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let from_table_name = from_table.map_or("<unknown>", |table| table.name.as_str());
    let to_table_name = to_table.map_or("<unknown>", |table| table.name.as_str());
    format!(
        "### Relation `{}`\n\n- Target: `{}.{}`\n- Foreign key: `{}.{}` ({}) -> `{}.{}` ({})",
        relation.name,
        relation.table.schema,
        relation.table.name,
        from_table.map_or("<unknown>", |table| table.schema.as_str()),
        from_table_name,
        qualify_columns(from_table_name, &from_columns),
        to_table.map_or("<unknown>", |table| table.schema.as_str()),
        to_table_name,
        qualify_columns(to_table_name, &to_columns)
    )
}

fn qualify_columns(table: &str, columns: &str) -> String {
    columns
        .split(", ")
        .filter(|column| !column.is_empty())
        .map(|column| format!("{table}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

enum CompletionContext {
    Root,
    Table(TableId),
}

fn selection_context(
    source_file: &SourceFile,
    catalog: &Catalog,
    byte: usize,
) -> CompletionContext {
    for definition in source_file.definitions() {
        match definition {
            Definition::Query(query) => {
                for selection in &query.selections {
                    if !contains(selection.range, byte) {
                        continue;
                    }
                    let Some(table) = catalog.table_ref(&selection.name.text) else {
                        return CompletionContext::Root;
                    };
                    return nested_context(catalog, table.id, &selection.selections, byte)
                        .unwrap_or(CompletionContext::Table(table.id));
                }
            }
            Definition::Fragment(fragment) => {
                if !contains(fragment.range, byte) {
                    continue;
                }
                let Some(on) = &fragment.on else {
                    return CompletionContext::Root;
                };
                let Some(table) = catalog.table_ref(&on.text) else {
                    return CompletionContext::Root;
                };
                return nested_context(catalog, table.id, &fragment.selections, byte)
                    .unwrap_or(CompletionContext::Table(table.id));
            }
        }
    }
    CompletionContext::Root
}

fn nested_context(
    catalog: &Catalog,
    table: TableId,
    selections: &[Selection],
    byte: usize,
) -> Option<CompletionContext> {
    for selection in selections {
        if !contains(selection.range, byte) {
            continue;
        }
        return match catalog.check_field(table, &selection.name.text) {
            FieldCheckResult::Relation(related) => {
                nested_context(catalog, related.table.id, &selection.selections, byte)
                    .or(Some(CompletionContext::Table(related.table.id)))
            }
            FieldCheckResult::Column(_)
            | FieldCheckResult::NotFound
            | FieldCheckResult::AmbiguousRelation { .. } => Some(CompletionContext::Table(table)),
        };
    }
    None
}

fn field_completions(catalog: &Catalog, table: TableId) -> Vec<CompletionItem> {
    let mut completions = Vec::new();
    completions.extend(
        catalog
            .columns_for_table(table)
            .map(|column| CompletionItem {
                label: column.name.clone(),
                kind: CompletionKind::Column,
                detail: Some(column.data_type.as_str().to_string()),
            }),
    );
    completions.extend(
        catalog
            .relation_fields_for_table(table)
            .into_iter()
            .map(|relation| CompletionItem {
                label: completion_table_ref(&relation.table.schema, relation.name),
                kind: CompletionKind::Relation,
                detail: Some(format!(
                    "relation to {}.{}",
                    relation.table.schema, relation.table.name
                )),
            }),
    );
    completions.sort_by(|left, right| left.label.cmp(&right.label));
    completions.dedup_by(|left, right| left.label == right.label);
    completions
}

fn completion_table_ref(schema: &str, table: &str) -> String {
    if schema == Catalog::DEFAULT_SCHEMA {
        table.to_string()
    } else {
        format!("{schema}.{table}")
    }
}

fn contains(range: dsql_core::TextRange, byte: usize) -> bool {
    (range.start as usize) <= byte && byte <= (range.end as usize)
}

fn apply_text_edits(rope: &mut Rope, edits: Vec<TextEdit>) {
    for edit in edits {
        if let Some(range) = edit.range {
            let start = position_to_byte(rope, range.start);
            let end = position_to_byte(rope, range.end);
            rope.remove(start..end);
            rope.insert(start, &edit.text);
        } else {
            *rope = Rope::from_str(&edit.text);
        }
    }
}

fn position_to_byte(rope: &Rope, position: TextPosition) -> usize {
    let line_count = rope.len_lines(LineType::LF_CR);
    let line = (position.line as usize).min(line_count.saturating_sub(1));
    let line_start = rope.line_to_byte_idx(line, LineType::LF_CR);
    let line_end = if line + 1 < line_count {
        rope.line_to_byte_idx(line + 1, LineType::LF_CR)
    } else {
        rope.len()
    };
    let line_slice = rope.slice(line_start..line_end);
    let utf16 = (position.character as usize).min(line_slice.len_utf16());
    line_start + line_slice.utf16_to_byte_idx(utf16)
}

pub fn analyze_source(source: SourceSnapshot) -> AnalysisResult {
    let parse = parse_source(source);
    let mut interner = Interner::default();
    let lower = lower_file(&parse.source_file, &mut interner);
    let check = check_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    let lint = lint_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    let plan = plan_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    AnalysisResult {
        parse,
        lower,
        check,
        lint,
        plan,
    }
}

pub fn collect_diagnostics(analysis: &AnalysisResult) -> Vec<Diagnostic> {
    collect_diagnostics_parts(
        &analysis.parse.diagnostics,
        &analysis.lower.diagnostics,
        &analysis.check.diagnostics,
        &analysis.lint.diagnostics,
    )
}

fn collect_diagnostics_parts(
    parse: &[Diagnostic],
    lower: &[Diagnostic],
    check: &[Diagnostic],
    lint: &[Diagnostic],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(parse.iter().cloned());
    diagnostics.extend(lower.iter().cloned());
    diagnostics.extend(check.iter().cloned());
    diagnostics.extend(lint.iter().cloned());
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsql_core::parse_source;

    fn parsed_source(source: &str) -> SourceFile {
        let parsed = parse_source(source.into());
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            parsed.diagnostics
        );
        parsed.source_file
    }

    #[test]
    fn completions_include_exact_relation_names() {
        let source = "query Q { posts {  } }";
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let byte = source.find("  }").unwrap() + 1;

        let completions = completions_at(&source_file, &catalog, byte);

        assert!(completions.iter().any(|completion| {
            completion.label == "users" && completion.kind == CompletionKind::Relation
        }));
        assert!(
            !completions
                .iter()
                .any(|completion| completion.label == "user")
        );
    }

    #[test]
    fn root_completions_qualify_non_public_tables() {
        let source = "query Q {  }";
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let byte = source.find("  }").unwrap() + 1;

        let completions = completions_at(&source_file, &catalog, byte);

        assert!(completions.iter().any(|completion| {
            completion.label == "users" && completion.kind == CompletionKind::Table
        }));
        assert!(completions.iter().any(|completion| {
            completion.label == "other_schema.users" && completion.kind == CompletionKind::Table
        }));
    }

    #[test]
    fn completions_and_hover_work_inside_fragments() {
        let source = "fragment UserFields on public.users {\n  posts {\n    title\n  }\n}";
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let completion_byte = source.find("  posts").unwrap() + 1;
        let hover_byte = source.find("title").unwrap();

        let completions = completions_at(&source_file, &catalog, completion_byte);
        let hover = hover_at(&source_file, &catalog, hover_byte).unwrap();

        assert!(completions.iter().any(|completion| {
            completion.label == "posts" && completion.kind == CompletionKind::Relation
        }));
        assert_eq!(hover.label, "title");
        assert_eq!(hover.detail, "column: text");
        assert!(hover.markdown.contains("Primary key: no"));
        assert!(hover.markdown.contains("Indexed: no"));
    }

    #[test]
    fn hover_and_completions_support_qualified_names() {
        let source = "query Q { public.users { public.posts { title } } }";
        let source_file = parsed_source(source);
        let catalog = Catalog::hardcoded();
        let completion_byte = source.find("public.posts").unwrap() - 1;
        let hover_byte = source.find("public.posts").unwrap();

        let completions = completions_at(&source_file, &catalog, completion_byte);
        let hover = hover_at(&source_file, &catalog, hover_byte).unwrap();

        assert!(completions.iter().any(|completion| {
            completion.label == "posts" && completion.kind == CompletionKind::Relation
        }));
        assert_eq!(hover.label, "public.posts");
        assert_eq!(hover.detail, "relation: public.posts");
        assert!(hover.markdown.contains("Foreign key"));
        assert!(hover.markdown.contains("posts.user_id"));
        assert!(hover.markdown.contains("users.id"));
    }

    #[test]
    fn semantic_tokens_classify_schema_tables_relations_and_columns() {
        let source = "query Q { public.users { id posts { title } } }";
        let parse = parse_source(source.into());
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        let catalog = Catalog::hardcoded();

        let tokens = semantic_tokens_at(&parse, &catalog);

        assert!(tokens.iter().any(|token| {
            token.kind == SemanticTokenKind::Schema
                && parse.source.text(token.range).as_ref() == "public"
        }));
        assert!(tokens.iter().any(|token| {
            token.kind == SemanticTokenKind::Table
                && parse.source.text(token.range).as_ref() == "users"
        }));
        assert!(tokens.iter().any(|token| {
            token.kind == SemanticTokenKind::Relation
                && parse.source.text(token.range).as_ref() == "posts"
        }));
        assert!(tokens.iter().any(|token| {
            token.kind == SemanticTokenKind::Column
                && parse.source.text(token.range).as_ref() == "title"
        }));
    }
}
