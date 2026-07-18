//! The bowl-level entity schema: the single source of truth for every
//! entity shape the dsql bowl may see.
//!
//! Shapes are *defined* here and *referenced* everywhere else — output
//! declarations select subsets (`Commands<(dsql_schema::Diagnostic,)>`),
//! strict spawns match against them, and the commit-time conformance
//! check enforces them by name. Adding a component to an entity kind
//! means editing one field here.

use bowl::{DerivedFrom, Singleton};

use crate::catalog::{CatalogSnapshot, CatalogSourceRoot};
use crate::embedding::{ExtractionRegistry, ResolvedEmbeddedExpression};
use crate::entities::aggregate::{AggregateTransformFact, ResolvedAggregate};
use crate::entities::clause::ClauseFact;
use crate::entities::definition::{DefDecl, DefIndex, FragmentKey, FragmentTarget};
use crate::entities::directive::DirectiveFact;
use crate::entities::document::ParsedFile;
use crate::entities::field_selection::FieldSel;
use crate::entities::fragment_spread::{ResolvedSpread, SpreadDecl};
use crate::entities::policy::{
    CompiledPolicyIndex, PolicyBodyIndex, PolicyDecl, PolicyIndex, PolicyPlanIndex,
};
use crate::entities::variable::{
    DefinitionVariables, DuplicateAnonymousBinding, VariableBinding, VariableUse,
};
use crate::facts::{
    BelongsToFile, ChildOf, Children, DefKey, Diagnostic, DiagnosticCode, DiagnosticSource,
    DiagnosticsDemand, NodeKey, PlanDemand, PlanKey, Severity, Span, SqlDemand, VariablesDemand,
};
use crate::lint::LintConfig;
use crate::plan::{FragmentPlanFact, OperationSeed, QueryPlanFact};
use crate::resolution::{
    FieldResolutions, ResolutionOf, ResolvedClause, ResolvedFragmentTarget, ResolvedSelection,
};
use crate::service::completion::{
    CompletionCandidate, CompletionContext, CompletionList, CompletionRequest,
    DirectiveCompletionContext,
};
use crate::service::definition::{DefinitionRequest, DefinitionTarget};
use crate::service::hover::{
    Cursor, HoverCandidate, HoverEnriched, HoverInfo, HoverRequest, Position, RequestKey,
};
use crate::service::semantic_tokens::{TokenChunk, TokensDemand};
use crate::source::{
    AnalysisResidency, BelongsToHost, CallsiteSpan, ContentSpan, DsqlDocument, EmbeddingHost,
    ExtractionResolver, FilePath, OpenBuffer, ResolutionScope, ScopeDocuments, ScopeImports,
    SourceOffset, SourceText,
};
use crate::sql::{GeneratedSqlFact, SqlOptions};

#[derive(bowl::Schema)]
pub struct DsqlSchema {
    // Base inputs: caller-inserted entity kinds. Base writes are not
    // conformance-checked (the dynamic boundary), but the schema must
    // still name them — it closes the component universe that presence
    // bitmaps and registration analyses are laid out over.
    dsql_file: (
        FilePath,
        SourceText,
        DsqlDocument,
        SourceOffset,
        ResolutionScope,
    ),
    host_file: (
        FilePath,
        SourceText,
        EmbeddingHost,
        ExtractionResolver,
        ResolutionScope,
    ),
    /// Editor-owned marker stamped externally on open documents.
    open_buffer: (OpenBuffer,),
    catalog: (Singleton<CatalogSnapshot>, CatalogSnapshot),
    catalog_source_root: (Singleton<CatalogSourceRoot>, CatalogSourceRoot),
    scope_imports: (Singleton<ScopeImports>, ScopeImports),
    scope_documents: (Singleton<ScopeDocuments>, ScopeDocuments),
    lint_config: (Singleton<LintConfig>, LintConfig),
    extraction_registry: (Singleton<ExtractionRegistry>, ExtractionRegistry),
    sql_options: (Singleton<SqlOptions>, SqlOptions),
    diagnostics_demand: (Singleton<DiagnosticsDemand>, DiagnosticsDemand),
    analysis_residency: (Singleton<AnalysisResidency>, AnalysisResidency),
    variables_demand: (Singleton<VariablesDemand>, VariablesDemand),
    plan_demand: (Singleton<PlanDemand>, PlanDemand),
    sql_demand: (Singleton<SqlDemand>, SqlDemand),
    tokens_demand: (Singleton<TokensDemand>, TokensDemand),
    hover_request: (HoverRequest, FilePath, Position),
    completion_request: (CompletionRequest, FilePath, Position),
    definition_request: (DefinitionRequest, FilePath, Position),
    // Lowered syntax facts, one shape per language entity. `ChildOf` is
    // optional: error recovery orphans descendants, and definitions are
    // roots. The engine maintains the `Children`/`FieldResolutions`
    // inverses.
    def: (
        NodeKey,
        DefDecl,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<FragmentKey>,
        Option<FragmentTarget>,
    ),
    policy_definition: (
        NodeKey,
        PolicyDecl,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
    ),
    field_selection: (
        NodeKey,
        FieldSel,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    aggregate_transform: (
        NodeKey,
        AggregateTransformFact,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    spread: (
        NodeKey,
        SpreadDecl,
        FragmentKey,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    clause: (
        NodeKey,
        ClauseFact,
        Span,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    /// Engine-maintained relationship inverses (ownerless base writes,
    /// named so the component universe closes over them).
    children: (Children,),
    field_resolutions: (FieldResolutions,),
    directive: (
        NodeKey,
        DirectiveFact,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    variable_use: (
        NodeKey,
        VariableUse,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    // Derived analysis facts.
    region: (
        SourceText,
        BelongsToHost,
        SourceOffset,
        CallsiteSpan,
        ContentSpan,
        DsqlDocument,
        ResolutionScope,
        DerivedFrom,
    ),
    /// Stamped onto the document entity by the parse system.
    parsed_file: (ParsedFile,),
    resolved_embedded_expression: (ResolvedEmbeddedExpression, BelongsToFile, DerivedFrom),
    diagnostic: (
        Diagnostic,
        Span,
        Severity,
        DiagnosticSource,
        DiagnosticCode,
        BelongsToFile,
        DerivedFrom,
    ),
    def_index: (Singleton<DefIndex>, DefIndex, Option<PolicyPlanIndex>),
    policy_index: (
        Singleton<PolicyIndex>,
        PolicyIndex,
        Option<CompiledPolicyIndex>,
    ),
    policy_body_index: (Singleton<PolicyBodyIndex>, PolicyBodyIndex),
    resolved_selection: (ResolvedSelection, ResolutionOf, BelongsToFile, DerivedFrom),
    resolved_aggregate: (ResolvedAggregate, BelongsToFile, DerivedFrom),
    resolved_clause: (ResolvedClause, BelongsToFile, DerivedFrom),
    resolved_fragment_target: (ResolvedFragmentTarget, BelongsToFile, DerivedFrom),
    resolved_spread: (ResolvedSpread, BelongsToFile, DerivedFrom),
    definition_variables: (DefinitionVariables, NodeKey, BelongsToFile, DerivedFrom),
    variable_binding: (VariableBinding, DefKey, Span, BelongsToFile, DerivedFrom),
    duplicate_anonymous_binding: (
        DuplicateAnonymousBinding,
        DefKey,
        Span,
        BelongsToFile,
        DerivedFrom,
    ),
    query_plan: (
        QueryPlanFact,
        OperationSeed,
        DefKey,
        BelongsToFile,
        DerivedFrom,
        Option<PlanKey>,
    ),
    fragment_plan: (FragmentPlanFact, DefKey, BelongsToFile, DerivedFrom),
    generated_sql: (GeneratedSqlFact, PlanKey, BelongsToFile, DerivedFrom),
    token_chunk: (TokenChunk, BelongsToFile, DerivedFrom),
    // The request/response services: candidates per (request, fact) pair,
    // the answer scaffold stamped on the request entity (optionals only
    // present when the request resolved to a document).
    hover_candidate: (HoverCandidate, RequestKey, DerivedFrom),
    completion_candidate: (CompletionCandidate, RequestKey, DerivedFrom),
    hover_answer: (
        RequestKey,
        HoverEnriched,
        HoverInfo,
        Option<Cursor>,
        Option<BelongsToFile>,
    ),
    completion_answer: (
        RequestKey,
        CompletionList,
        Option<CompletionContext>,
        Option<DirectiveCompletionContext>,
    ),
    definition_enriched: (Cursor, BelongsToFile),
    definition_answer: (DefinitionTarget,),
}

/// Everything the shared lowering walk may spawn — the *normalized AST*:
/// there is no owned tree value, the fact graph (these shapes plus the
/// `ChildOf`/`Children` relationships) is the syntax representation
/// consumers join against. The group every [`LowerStage`] implementation
/// declares through its `Commands`.
///
/// [`LowerStage`]: crate::entity::LowerStage
pub type AstFacts = (
    dsql_schema::Def,
    dsql_schema::PolicyDefinition,
    dsql_schema::FieldSelection,
    dsql_schema::AggregateTransform,
    dsql_schema::Spread,
    dsql_schema::Clause,
    dsql_schema::Directive,
    dsql_schema::VariableUse,
);
