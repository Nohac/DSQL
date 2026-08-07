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
use crate::embedding::{
    EmbeddedDefinitionCandidate, EmbeddedDefinitionCandidateOf, EmbeddedDefinitionCandidates,
    EmbeddedExpressionSiteKey, EmbeddedExpressionSiteRoot, ExtractionRegistry,
    ResolvedEmbeddedExpression,
};
use crate::entities::aggregate::{
    AggregateResolutionOf, AggregateResolutions, AggregateTransformFact, ResolvedAggregate,
};
use crate::entities::clause::ClauseFact;
use crate::entities::context::{
    ContextDecl, ContextDeclarationContext, ContextDeclarationContextOf,
    ContextDeclarationContexts, ContextDeclarationKey, ContextDeclarationNavigation,
    ContextDeclarationPeer, ContextDeclarationPeerOf, ContextDeclarationPeers,
    ContextDeclarationSemantics, ContextDeclarationSiteKey, ContextDeclarationSiteRoot,
    ContextNameKey, ContextSource, ContextUseCandidate, ContextUseCandidateOf,
    ContextUseCandidates, ContextUseContext, ContextUseContextOf, ContextUseContexts,
    ContextUseResolutionOf, ContextUseResolutions, ContextUseSiteKey, ContextUseSiteRoot,
    ResolvedContextUse,
};
use crate::entities::definition::{
    DefDecl, DefinitionNameKey, DefinitionNavigation, DefinitionPath, DefinitionSemantics,
    DefinitionSiteContext, DefinitionSiteContextOf, DefinitionSiteContexts, DefinitionSiteKey,
    DefinitionSiteRoot, FragmentKey, FragmentTarget, ImportedQueryPeer, ImportedQueryPeerOf,
    ImportedQueryPeers, VisibleDefinitionCandidate, VisibleDefinitionCandidateOf,
    VisibleDefinitionCandidates,
};
use crate::entities::directive::DirectiveFact;
use crate::entities::document::ParsedFile;
use crate::entities::expansion::{
    ClosingSpreadCycles, DependsOnSemanticGroup, ExpansionBodies, ExpansionBody, ExpansionBodyOf,
    ExpansionCycle, ExpansionCycleAt, ExpansionCycleOf, ExpansionCycles, ExpansionOccurrence,
    ExpansionOccurrenceOf, ExpansionOccurrences, SemanticDefinitionKey, SemanticDependents,
    SpreadResolutionOf, SpreadResolutions, SpreadSiteGroup, SpreadSiteRoot,
};
use crate::entities::field_selection::FieldSel;
use crate::entities::fragment_spread::{
    FragmentCandidateKey, FragmentSemantics, ResolvedSpread, ResolvedSpreadNavigation, SpreadDecl,
    VisibleFragmentCandidate, VisibleFragmentCandidateOf, VisibleFragmentCandidates,
    VisibleFragmentNavigation,
};
use crate::entities::policy::{
    CompiledPolicy, CompiledPolicyIndex, DefinitionPolicies, DefinitionPolicy, DefinitionPolicyOf,
    DefinitionPolicySurface, DefinitionShapePolicies, DefinitionShapePolicy,
    DefinitionShapePolicyOf, PolicyCompileProblems, PolicyContextCandidate,
    PolicyContextCandidateOf, PolicyContextCandidates, PolicyContextReference, PolicyDecl,
    PolicyIndex, PolicyNameKey, PolicyNavigation, PolicyPeer, PolicyPeerOf, PolicyPeers,
    PolicyReference, PolicyRegistryMember, PolicyRegistryMemberOf, PolicyRegistryMembers,
    PolicyRegistryRoot, PolicySiteContext, PolicySiteContextOf, PolicySiteContexts, PolicySiteKey,
    PolicySiteRoot, VisiblePolicyCandidate, VisiblePolicyCandidateOf, VisiblePolicyCandidates,
};
use crate::entities::variable::{
    DefinitionInputRewrites, DefinitionVariableOwner, DefinitionVariables,
    DuplicateAnonymousBinding, VariableBinding, VariableProblem, VariableUse,
};
use crate::facts::{
    BelongsToFile, ChildOf, Children, DefKey, Diagnostic, DiagnosticCode, DiagnosticSource,
    DiagnosticsDemand, NodeKey, PlanDemand, PlanKey, SemanticMemberOf, SemanticMembers,
    SemanticRoot, Severity, Span, SqlDemand, VariablesDemand,
};
use crate::lint::LintConfig;
use crate::plan::{FragmentPlanFact, OperationSeed, QueryPlanFact};
use crate::resolution::{
    ClauseResolutionOf, ClauseResolutions, FieldResolutions, ResolutionOf, ResolvedClause,
    ResolvedFragmentTarget, ResolvedSelection, SelectionResolutionOf, SelectionResolutions,
};
use crate::service::completion::{
    CompletionCandidate, CompletionContext, CompletionList, CompletionRequest,
    DirectiveCompletionContext, PolicyCompletionContext,
};
use crate::service::definition::{DefinitionRequest, DefinitionTarget};
use crate::service::hover::{
    Cursor, HoverCandidate, HoverEnriched, HoverInfo, HoverRequest, Position, RequestKey,
};
use crate::service::semantic_tokens::{TokenChunk, TokensDemand};
use crate::source::{
    AnalysisResidency, BelongsToHost, CallsiteSpan, ContentSpan, DocumentPath, DsqlDocument,
    EmbeddingHost, ExtractionResolver, FilePath, OpenBuffer, ResolutionScope, ScopeDocuments,
    ScopeImports, SourceOffset, SourceText,
};
use crate::sql::{GeneratedSqlFact, SqlOptions};

#[expect(
    clippy::type_complexity,
    reason = "schema tuples are the explicit fact-shape contract"
)]
#[derive(bowl::Schema)]
pub struct DsqlSchema {
    // Base inputs: caller-inserted entity kinds. Base writes are not
    // conformance-checked (the dynamic boundary), but the schema must
    // still name them — it closes the component universe that presence
    // bitmaps and registration analyses are laid out over.
    dsql_file: (
        FilePath,
        DocumentPath,
        SourceText,
        DsqlDocument,
        SourceOffset,
        ResolutionScope,
    ),
    host_file: (
        FilePath,
        DocumentPath,
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
        Option<SemanticDefinitionKey>,
        DefinitionNameKey,
        DefinitionPath,
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
        PolicyNameKey,
        PolicyNavigation,
        Option<PolicySiteKey>,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
    ),
    context_declaration: (
        NodeKey,
        ContextDecl,
        Option<ContextDeclarationKey>,
        ContextNameKey,
        ContextSource,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
    ),
    context_declaration_semantic_projection: (
        ContextDeclarationSemantics,
        ContextDeclarationKey,
        ContextNameKey,
        ContextDeclarationSiteKey,
        DerivedFrom,
    ),
    context_declaration_navigation_projection: (
        ContextDeclarationNavigation,
        ContextDeclarationKey,
        ContextNameKey,
        ContextDeclarationSiteKey,
        DerivedFrom,
    ),
    context_declaration_site: (ContextDeclarationSiteRoot, ContextDeclarationSiteKey),
    context_declaration_contexts: (ContextDeclarationContexts,),
    context_declaration_context: (
        ContextDeclarationContext,
        ContextDeclarationContextOf,
        ContextDeclarationKey,
        ContextNameKey,
        DerivedFrom,
    ),
    context_declaration_peers: (ContextDeclarationPeers,),
    context_declaration_peer: (
        ContextDeclarationPeer,
        ContextDeclarationPeerOf,
        DerivedFrom,
    ),
    semantic_group: (
        SemanticRoot,
        DerivedFrom,
        Option<NodeKey>,
        Option<SemanticDefinitionKey>,
        Option<DefinitionPolicySurface>,
    ),
    spread_site: (NodeKey, SpreadSiteRoot, DerivedFrom),
    semantic_members: (SemanticMembers,),
    context_use_resolutions: (ContextUseResolutions,),
    context_use_site: (ContextUseSiteRoot, ContextUseSiteKey),
    context_use_contexts: (ContextUseContexts,),
    context_use_context: (
        ContextUseContext,
        ContextUseContextOf,
        ContextUseSiteKey,
        ContextNameKey,
        DerivedFrom,
    ),
    context_use_candidates: (ContextUseCandidates,),
    context_use_candidate: (ContextUseCandidate, ContextUseCandidateOf, DerivedFrom),
    spread_resolutions: (SpreadResolutions,),
    semantic_dependents: (SemanticDependents,),
    expansion_occurrences: (ExpansionOccurrences,),
    expansion_cycles: (ExpansionCycles,),
    closing_spread_cycles: (ClosingSpreadCycles,),
    visible_fragment_candidates: (VisibleFragmentCandidates,),
    expansion_bodies: (ExpansionBodies,),
    field_selection: (
        NodeKey,
        FieldSel,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
        Option<SemanticMemberOf>,
    ),
    aggregate_transform: (
        NodeKey,
        AggregateTransformFact,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
        Option<SemanticMemberOf>,
    ),
    spread: (
        NodeKey,
        SpreadDecl,
        FragmentKey,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
        Option<SemanticMemberOf>,
    ),
    clause: (
        NodeKey,
        ClauseFact,
        Span,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
        Option<SemanticMemberOf>,
    ),
    /// Engine-maintained relationship inverses (ownerless base writes,
    /// named so the component universe closes over them).
    children: (Children,),
    field_resolutions: (FieldResolutions,),
    selection_resolutions: (SelectionResolutions,),
    clause_resolutions: (ClauseResolutions,),
    aggregate_resolutions: (AggregateResolutions,),
    directive: (
        NodeKey,
        DirectiveFact,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
        Option<SemanticMemberOf>,
    ),
    variable_use: (
        NodeKey,
        VariableUse,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
        Option<SemanticMemberOf>,
        Option<ContextUseSiteKey>,
    ),
    // Derived analysis facts.
    region: (
        SourceText,
        BelongsToHost,
        SourceOffset,
        CallsiteSpan,
        ContentSpan,
        DocumentPath,
        DsqlDocument,
        ResolutionScope,
        DerivedFrom,
    ),
    /// Stamped onto the document entity by the parse system.
    parsed_file: (ParsedFile,),
    definition_semantic_projection: (
        DefinitionSemantics,
        DefinitionNameKey,
        DefinitionSiteKey,
        SemanticDefinitionKey,
        BelongsToFile,
        DerivedFrom,
    ),
    definition_navigation_projection: (
        DefinitionNavigation,
        DefinitionNameKey,
        DefinitionSiteKey,
        SemanticDefinitionKey,
        DerivedFrom,
    ),
    // Stable relationship owners deliberately omit DerivedFrom. Their
    // producing invocations retire them; candidate/context children carry
    // revision-sensitive lifetime anchors.
    definition_site: (DefinitionSiteRoot, DefinitionSiteKey, SemanticDefinitionKey),
    definition_site_contexts: (DefinitionSiteContexts,),
    definition_site_context: (DefinitionSiteContext, DefinitionSiteContextOf, DerivedFrom),
    visible_definition_candidates: (VisibleDefinitionCandidates,),
    visible_definition_candidate: (
        VisibleDefinitionCandidate,
        VisibleDefinitionCandidateOf,
        DerivedFrom,
    ),
    imported_query_peers: (ImportedQueryPeers,),
    imported_query_peer: (ImportedQueryPeer, ImportedQueryPeerOf, DerivedFrom),
    embedded_expression_site: (
        EmbeddedExpressionSiteRoot,
        EmbeddedExpressionSiteKey,
        BelongsToFile,
    ),
    embedded_definition_candidates: (EmbeddedDefinitionCandidates,),
    embedded_definition_candidate: (
        EmbeddedDefinitionCandidate,
        EmbeddedDefinitionCandidateOf,
        DerivedFrom,
    ),
    resolved_embedded_expression: (ResolvedEmbeddedExpression, BelongsToFile),
    diagnostic: (
        Diagnostic,
        Span,
        Severity,
        DiagnosticSource,
        DiagnosticCode,
        BelongsToFile,
        DerivedFrom,
    ),
    policy_site: (
        PolicySiteRoot,
        PolicySiteKey,
        PolicyNameKey,
        Option<CompiledPolicy>,
        Option<PolicyCompileProblems>,
    ),
    policy_site_contexts: (PolicySiteContexts,),
    policy_site_context: (
        PolicySiteContext,
        PolicySiteContextOf,
        PolicyNameKey,
        DerivedFrom,
    ),
    policy_reference: (PolicyReference, PolicyNameKey, PolicySiteKey, DerivedFrom),
    policy_context_reference: (
        PolicyContextReference,
        ContextNameKey,
        PolicySiteKey,
        DerivedFrom,
    ),
    policy_context_candidates: (PolicyContextCandidates,),
    policy_context_candidate: (
        PolicyContextCandidate,
        PolicyContextCandidateOf,
        DerivedFrom,
    ),
    visible_policy_candidates: (VisiblePolicyCandidates,),
    visible_policy_candidate: (
        VisiblePolicyCandidate,
        VisiblePolicyCandidateOf,
        DerivedFrom,
    ),
    policy_peers: (PolicyPeers,),
    policy_peer: (PolicyPeer, PolicyPeerOf, DerivedFrom),
    policy_registry: (
        Singleton<PolicyRegistryRoot>,
        PolicyRegistryRoot,
        Option<PolicyIndex>,
        Option<CompiledPolicyIndex>,
    ),
    policy_registry_members: (PolicyRegistryMembers,),
    policy_registry_member: (PolicyRegistryMember, PolicyRegistryMemberOf, DerivedFrom),
    definition_policies: (DefinitionPolicies,),
    definition_policy: (DefinitionPolicy, DefinitionPolicyOf, DerivedFrom),
    definition_shape_policies: (DefinitionShapePolicies,),
    definition_shape_policy: (DefinitionShapePolicy, DefinitionShapePolicyOf, DerivedFrom),
    resolved_context_use: (
        ResolvedContextUse,
        ContextUseResolutionOf,
        BelongsToFile,
        Option<ContextDeclarationKey>,
    ),
    resolved_selection: (
        ResolvedSelection,
        ResolutionOf,
        SelectionResolutionOf,
        BelongsToFile,
        DerivedFrom,
    ),
    resolved_aggregate: (
        ResolvedAggregate,
        AggregateResolutionOf,
        BelongsToFile,
        DerivedFrom,
    ),
    resolved_clause: (
        ResolvedClause,
        ClauseResolutionOf,
        NodeKey,
        BelongsToFile,
        DerivedFrom,
    ),
    resolved_fragment_target: (ResolvedFragmentTarget, BelongsToFile, DerivedFrom),
    visible_fragment_candidate: (
        VisibleFragmentCandidate,
        FragmentCandidateKey,
        VisibleFragmentCandidateOf,
        DerivedFrom,
    ),
    fragment_semantic_projection: (
        FragmentSemantics,
        FragmentKey,
        SemanticDefinitionKey,
        DerivedFrom,
    ),
    visible_fragment_navigation: (VisibleFragmentNavigation, FragmentCandidateKey, DerivedFrom),
    resolved_spread: (
        ResolvedSpread,
        SpreadResolutionOf,
        BelongsToFile,
        DerivedFrom,
        SpreadSiteGroup,
        Option<FragmentCandidateKey>,
        Option<ResolvedSpreadNavigation>,
        Option<SemanticDefinitionKey>,
        Option<DependsOnSemanticGroup>,
    ),
    expansion_occurrence: (
        ExpansionOccurrence,
        SemanticDefinitionKey,
        ExpansionOccurrenceOf,
        DerivedFrom,
    ),
    expansion_cycle: (
        ExpansionCycle,
        ExpansionCycleOf,
        ExpansionCycleAt,
        DerivedFrom,
    ),
    expansion_body: (ExpansionBody, ExpansionBodyOf, DerivedFrom),
    definition_variables: (
        DefinitionVariables,
        DefinitionInputRewrites,
        DefinitionVariableOwner,
        NodeKey,
        SemanticDefinitionKey,
        DefKey,
        BelongsToFile,
        DerivedFrom,
    ),
    variable_binding: (VariableBinding, DefKey, Span, BelongsToFile, DerivedFrom),
    duplicate_anonymous_binding: (
        DuplicateAnonymousBinding,
        DefKey,
        Span,
        BelongsToFile,
        DerivedFrom,
    ),
    variable_problem: (VariableProblem, Span, BelongsToFile, DerivedFrom),
    query_plan: (
        QueryPlanFact,
        OperationSeed,
        // Direct join back to the lowered definition for editor services;
        // lowered definition entities do not carry DefKey.
        NodeKey,
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
        Option<DefKey>,
        Option<CompletionContext>,
        Option<DirectiveCompletionContext>,
        Option<PolicyCompletionContext>,
    ),
    definition_enriched: (Cursor, BelongsToFile),
    definition_answer: (DefinitionTarget,),
}

/// Everything the shared lowering walk may spawn or add — the *normalized AST*:
/// there is no owned tree value, the fact graph (these shapes plus the
/// `ChildOf`/`Children` relationships) is the syntax representation
/// consumers join against. The group every [`LowerStage`] implementation
/// declares through its `Commands`.
///
/// The six nested spawn shapes deliberately repeat their pre-edge tuples
/// instead of using the generated schema aliases. [`SemanticMemberOf`] is
/// optional on all six runtime shapes, and [`SemanticDefinitionKey`] is
/// optional on both definition and semantic-group shapes. Listing those
/// generated aliases here would give the walker's untyped central writes
/// several valid declaration witnesses and make type inference ambiguous. The
/// final manual tuple declares each component once. This trades away static
/// proof that the edge and key target permitting shapes; the semantic-ownership
/// integration fixture therefore exercises every tuple below through
/// commit-time schema conformance. Future cross-cutting optional components
/// spanning multiple shapes require the same output-declaration split.
///
/// [`LowerStage`]: crate::entity::LowerStage
pub type AstFacts = (
    (
        NodeKey,
        DefDecl,
        DefinitionNameKey,
        DefinitionPath,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<FragmentKey>,
        Option<FragmentTarget>,
    ),
    dsql_schema::PolicyDefinition,
    dsql_schema::ContextDeclaration,
    (SemanticRoot, DerivedFrom, Option<NodeKey>),
    dsql_schema::SpreadSite,
    (
        NodeKey,
        FieldSel,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    (
        NodeKey,
        AggregateTransformFact,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    (
        NodeKey,
        SpreadDecl,
        FragmentKey,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    (
        NodeKey,
        ClauseFact,
        Span,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    (
        NodeKey,
        DirectiveFact,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
    ),
    (
        NodeKey,
        VariableUse,
        ResolutionScope,
        BelongsToFile,
        DerivedFrom,
        Option<ChildOf>,
        Option<ContextUseSiteKey>,
    ),
    (Option<SemanticDefinitionKey>, Option<SemanticMemberOf>),
);
