use crate::analysis::collect_diagnostics_parts;
use crate::document::{FileId, RevisionId};
use dashmap::DashMap;
use dsql_core::{
    Catalog, CheckedFile, Diagnostic, Interner, LintedFile, LoweredFile, ParseResult, PlannedFile,
    SourceFile, SourceSnapshot, SyntaxTree, check_file_with_catalog, format_file,
    lint_file_with_catalog, lower_file, parse_source, plan_file_with_catalog,
};
use facet::Facet;
use picante::PicanteResult;
use ropey::Rope;
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
    pub(crate) fn set_catalog(&self, catalog: Catalog) -> PicanteResult<()> {
        CatalogInput::set(self, catalog)
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
        Ok(())
    }

    pub(crate) fn remove_source(&self, file: FileId) {
        self.source_inputs.remove(&file);
    }

    fn source_input(&self, file: FileId) -> Option<SourceInput> {
        self.source_inputs.get(&file).map(|source| *source)
    }

    pub(crate) async fn analysis(&self, file: FileId) -> Option<AnalysisResult> {
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

    pub(crate) async fn diagnostics(&self, file: FileId) -> PicanteResult<Vec<Diagnostic>> {
        let Some(source) = self.source_input(file) else {
            return Ok(Vec::new());
        };
        diagnostics_for_file(self, source).await
    }

    pub(crate) async fn formatted_text(&self, file: FileId) -> PicanteResult<Option<String>> {
        let Some(source) = self.source_input(file) else {
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
}
