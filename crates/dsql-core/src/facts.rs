//! Cross-cutting facts shared by every language entity: spans, diagnostics,
//! file anchoring, and demand markers.

use bowl::{Commands, Component, DerivedFrom, Entity, Singleton, SpawnsAs};

/// Byte range into the source text of one file. Positions (line/column)
/// are computed at protocol boundaries only.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// Whether `offset` lies inside this half-open byte range.
    pub fn contains(self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(range: std::ops::Range<usize>) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

/// Severity of a [`Diagnostic`] fact.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// Machine-readable diagnostic identity. Editor
/// integrations and tests key on these; messages are for humans only.
/// Variants are added as the checks that emit them land.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub enum DiagnosticCode {
    InvalidToken,
    UnexpectedToken,
    UnexpectedEof,
    DuplicateDefinition,
    UnsupportedDirective,
    UnknownDirective,
    DirectiveNotAllowed,
    MissingDirectiveArgument,
    UnknownDirectiveArgument,
    DuplicateDirectiveArgument,
    DirectiveArgumentTypeMismatch,
    EmbeddedExpressionShape,
    UnknownFragment,
    TableNotFound,
    AmbiguousTable,
    FieldNotFound,
    AmbiguousRelation,
    DuplicateOutputKey,
    OutputKeyTooLong,
    ScalarSelectionSet,
    ScalarClauses,
    RelationSelectionSet,
    FragmentTypeMismatch,
    CircularFragmentSpread,
    ClauseValueTypeMismatch,
    PredicateTypeMismatch,
    DuplicateAnonymousVariable,
    UnindexedJoinColumn,
    UnindexedScanColumn,
    UnindexedPredicateJoinColumn,
    SqlGeneration,
}

/// Which stage emitted a diagnostic.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub enum DiagnosticSource {
    Parse,
    Lower,
    Check,
    Lint,
    Plan,
    Generate,
    Format,
}

/// Stable identity of one CST rule node within one parse of one file.
/// Valid only for the lifetime of that parse: any text change re-lowers the
/// whole file, retiring every fact that carries one.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(hash)]
pub struct NodeKey {
    pub file: Entity,
    pub node: usize,
}

/// The definition entity a derived fact belongs to, as a join/scoop key:
/// plans and variable bindings carry the definition they derive from so
/// artifact assembly can group them without entity-graph walks.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct DefKey(pub Entity);

/// The plan entity a generated-SQL fact renders, as a join/scoop key.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
pub struct PlanKey(pub Entity);

/// Relationship edge from a lowered fact to the entity of its nearest
/// enclosing selection or definition — the selection tree as maintained
/// relationships. The engine keeps the [`Children`] inverse current on
/// the parent.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[component(hash)]
#[relationship(target = Children)]
pub struct ChildOf(pub Entity);

/// Engine-maintained inverse of [`ChildOf`]: every child of this entity,
/// ordered by entity id, fingerprinted over the ordered set — membership
/// is a revision-level fact. Sibling *source* order is still span order;
/// entity-id order only matches it on first derivation.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[relationship_target(relationship = ChildOf)]
pub struct Children(pub Vec<Entity>);

/// Human-readable diagnostic message. A diagnostic entity carries this plus
/// [`Severity`], [`Span`], [`BelongsToFile`], and `DerivedFrom` (ownership,
/// so stale diagnostics clean up when their source changes).
#[derive(Component, Hash, Debug, Clone, PartialEq, Eq)]
#[component(hash)]
pub struct Diagnostic(pub String);

/// Join key anchoring a derived fact to the file entity it came from.
/// Lets services and adapters filter facts per file with bound queries.
#[derive(Component, Hash, Debug, Clone, Copy, PartialEq, Eq)]
#[component(hash)]
pub struct BelongsToFile(pub Entity);

/// Demand marker: diagnostics systems gate on this singleton fact, so a
/// bowl that nobody asked diagnostics from never plans them. The LSP
/// inserts it when the editor goes idle — debounce as data, not timers.
#[derive(Component, Hash)]
#[component(hash)]
pub struct DiagnosticsDemand;

/// Demand marker for variable-binding inference (`variable` entity):
/// generate/metadata consumers and tests insert it; idle bowls infer nothing.
#[derive(Component, Hash)]
#[component(hash)]
pub struct VariablesDemand;

/// Demand marker for query planning (`plan::build`).
#[derive(Component, Hash)]
#[component(hash)]
pub struct PlanDemand;

/// Demand marker for SQL generation (`sql::generate`). SQL needs plans:
/// insert [`PlanDemand`] alongside.
#[derive(Component, Hash)]
#[component(hash)]
pub struct SqlDemand;

/// Arms the demand set one full generation needs (diagnostics gate the
/// write, variables/plans/SQL are its artifacts). One bundle instead of
/// four call-site inserts, so no adapter can settle an incomplete
/// generation pipeline; [`SqlDemand`]'s plan prerequisite is wired here
/// once.
pub async fn arm_generate_demands(bowl: &bowl::Bowl) {
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
    bowl.insert((Singleton::<PlanDemand>::new(), PlanDemand))
        .await;
    bowl.insert((Singleton::<SqlDemand>::new(), SqlDemand))
        .await;
}

/// Arms the demand set editor sessions need: diagnostics for publishing
/// and variables for hover.
pub async fn arm_editor_demands(bowl: &bowl::Bowl) {
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
}

/// Everything one diagnostic entity is made of. Every field is required so
/// no emitter can silently drop a component the LSP or tests key on.
pub struct DiagnosticFacts {
    /// The source facts whose change retires the diagnostic.
    pub derived_from: DerivedFrom,
    /// The file whose text [`DiagnosticFacts::span`] indexes.
    pub file: Entity,
    pub span: Span,
    pub severity: Severity,
    pub source: DiagnosticSource,
    pub code: DiagnosticCode,
    pub message: String,
}

/// Emits one diagnostic entity carrying the full diagnostic component set.
pub fn emit_diagnostic<S, M>(commands: &mut Commands<S>, facts: DiagnosticFacts)
where
    (
        DerivedFrom,
        BelongsToFile,
        Span,
        Severity,
        DiagnosticSource,
        DiagnosticCode,
        Diagnostic,
    ): SpawnsAs<S, M>,
{
    commands.insert((
        facts.derived_from,
        BelongsToFile(facts.file),
        facts.span,
        facts.severity,
        facts.source,
        facts.code,
        Diagnostic(facts.message),
    ));
}
