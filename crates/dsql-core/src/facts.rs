//! Cross-cutting facts shared by every language entity: spans, diagnostics,
//! file anchoring, and demand markers.

use bowl::{Commands, Component, DerivedFrom, Entity};

/// Byte range into the source text of one file. Positions (line/column)
/// are computed at protocol boundaries only.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
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

/// Link from a lowered fact to the [`NodeKey`] of its nearest enclosing
/// selection or definition — the flat encoding of the selection tree.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[component(hash)]
pub struct ParentKey(pub NodeKey);

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
pub fn emit_diagnostic(commands: &mut Commands, facts: DiagnosticFacts) {
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
