# Language Atoms

## Summary

A language atom is an indivisible compiler responsibility for one source-level
language concept. It ties a grammar rule to the typed source model and declares
which compiler, generation, and editor stages must account for that concept.

The goal is to prevent language drift. When a construct is added, removed, or
renamed, the compiler should fail to build until every relevant stage either
implements the construct or explicitly declares that the construct has no effect
there.

Atoms are not a plugin system. They are a static architecture tool for keeping
syntax, AST construction, formatting, lowering, checking, linting, planning,
generation, and editor support aligned.

## Motivation

Today a language feature can be partially implemented without becoming obvious.
For example, a construct can be parsed and formatted while checking, planning,
generation, completion, hover, and semantic tokens silently ignore it.

The current architecture already has useful stage boundaries:

- parsing and lowering are context-free;
- `scoped_program(env)` is the boundary where resolution context enters;
- checking, linting, planning, and generation consume scoped semantic data;
- adapters go through `ProjectHost`.

Language atoms should preserve those boundaries while making stage coverage
explicit.

## Non-Goals

- Atoms do not own project loading, source-unit membership, resolution maps, or
  scoped-program construction.
- Atoms do not load catalogs, config, or source text.
- Atoms do not expose Picante query internals to adapters.
- Atoms do not make the compiler dynamically extensible at runtime.
- Atoms do not replace tests. They make missing stage coverage visible at
  compile time, but tests still prove behavior.

## Terms

- **Language atom**: one source-level construct with a single ownership point,
  for example a directive, field selection, fragment spread, clause, variable,
  or expression form.
- **Grammar rule**: the parser rule that anchors an atom in source syntax.
- **Owned rule**: a grammar rule claimed by exactly one atom.
- **Internal rule**: a grammar rule used only to structure syntax and not owned
  directly by a language atom.
- **Stage coverage**: an implementation or explicit no-effect declaration for
  one atom at one stage.
- **No-effect declaration**: an intentional statement that an atom does not
  affect a stage.

## Core Contract

Each atom has one central declaration:

```rust
pub trait LanguageAtom {
    type GrammarRule;
    type Ast;
    type Lowered;

    const NAME: &'static str;
}
```

An atom declaration should be colocated with its stage implementations:

```rust
pub enum DirectiveAtom {}

impl LanguageAtom for DirectiveAtom {
    type GrammarRule = grammar_rule::Directive;
    type Ast = ast::Directive;
    type Lowered = lowered::Directive;

    const NAME: &'static str = "directive";
}
```

The declaration means:

- this atom owns the `directive` grammar rule;
- this atom has a typed AST representation;
- this atom has a lowered representation, or an explicit unit lowered
  representation if lowering only indexes names;
- every required stage must either implement this atom or explicitly declare
  no effect.

## Grammar Ownership

Grammar rules should be represented by generated marker types:

```rust
pub mod grammar_rule {
    pub enum Directive {}
    pub enum FieldSelection {}
    pub enum FragmentSpread {}
}
```

The generated marker type should be tied to the generated parser rule:

```rust
pub trait GrammarRuleMarker {
    const RULE: parser::Rule;
    const NAME: &'static str;
}

impl GrammarRuleMarker for grammar_rule::Directive {
    const RULE: parser::Rule = parser::Rule::Directive;
    const NAME: &'static str = "directive";
}
```

This gives rename/delete safety. If the grammar rule is renamed or removed,
`parser::Rule::Directive` no longer exists and the crate fails to compile.

Each owned rule also has one atom owner:

```rust
pub trait GrammarRuleOwner {
    type Atom: LanguageAtom;
}

impl GrammarRuleOwner for grammar_rule::Directive {
    type Atom = DirectiveAtom;
}
```

If another atom tries to claim the same grammar rule, Rust reports a conflicting
implementation.

Every grammar rule should be classified as one of:

```rust
impl GrammarRuleOwner for grammar_rule::Directive {
    type Atom = DirectiveAtom;
}

impl InternalGrammarRule for grammar_rule::FieldSuffix {
    const REASON: &'static str = "groups clauses, directives, and nested selections";
}
```

The grammar bridge should assert that every generated rule is either owned or
internal. This prevents newly added grammar rules from drifting without a
compiler ownership decision.

## Stage Coverage

Coverage traits should be small and stage-specific:

```rust
pub trait BuildsAst<A: LanguageAtom> {
    fn build_ast(&self, node: NodeRef) -> A::Ast;
}

pub trait FormatsAtom<A: LanguageAtom> {
    fn format_atom(&mut self, node: SyntaxNodeRef);
}

pub trait LowersAtom<A: LanguageAtom> {
    fn lower_atom(&mut self, ast: &A::Ast) -> A::Lowered;
}

pub trait ChecksAtom<A: LanguageAtom> {
    fn check_atom(&mut self, ast: &A::Ast, context: CheckContext<'_>);
}
```

Stages that may not apply to all atoms should require either a positive
implementation or a no-effect implementation:

```rust
pub trait PlansAtom<A: LanguageAtom> {}

pub trait NoPlanEffect<A: LanguageAtom> {
    const REASON: &'static str;
}
```

The atom contract should accept either:

```rust
impl PlansAtom<DirectiveAtom> for Planner {}
```

or:

```rust
impl NoPlanEffect<DirectiveAtom> for Planner {
    const REASON: &'static str = "directives are validation and editor metadata only";
}
```

Silent omission should not compile.

## Required Stages

The baseline atom contract should cover these stages:

```rust
pub trait AtomCoverage<A: LanguageAtom>
where
    A::GrammarRule: GrammarRuleMarker + GrammarRuleOwner<Atom = A>,
    AstBuilder: BuildsAst<A>,
    CstFormatter: FormatsAtom<A>,
    Lowerer: LowersAtom<A>,
    Checker: ChecksCoverage<A>,
    Linter: LintCoverage<A>,
    VariableInference: VariableCoverage<A>,
    Planner: PlanCoverage<A>,
    PostgresSqlGenerator: SqlCoverage<A>,
    MetadataGenerator: MetadataCoverage<A>,
    EditorFeatures: EditorCoverage<A>,
{
}
```

Coverage aliases can allow either implementation or no-effect declarations:

```rust
pub trait PlanCoverage<A: LanguageAtom> {}

impl<A, T> PlanCoverage<A> for T
where
    A: LanguageAtom,
    T: PlansAtom<A>,
{
}

impl<A, T> PlanCoverage<A> for T
where
    A: LanguageAtom,
    T: NoPlanEffect<A>,
{
}
```

If overlapping blanket impls become awkward, use sealed marker types generated
by the atom declaration macro instead of public blanket impls.

## Atom Declaration Macro

The user-facing declaration should be compact:

```rust
language_atom! {
    DirectiveAtom {
        name: "directive",
        grammar_rule: Directive,
        ast: ast::Directive,
        lowered: lowered::Directive,

        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("directives do not produce lints"),
        variables: no_effect("directives do not bind variables"),
        plan: no_effect("directives do not change execution plans"),
        sql: no_effect("directives do not reach SQL generation"),
        metadata: no_effect("directives are not emitted in public metadata"),
        editor: required,
    }
}
```

The macro should generate:

- the `LanguageAtom` implementation;
- the grammar-rule owner implementation;
- compile-time stage coverage assertions;
- a single atom descriptor for tests and debug output.

## File Layout

Atoms should be vertical slices:

```text
crates/core/src/language/
  mod.rs
  grammar.rs
  stages.rs
  atoms/
    directive.rs
    field_selection.rs
    fragment_spread.rs
    clause.rs
    variable.rs
```

An atom file owns feature-specific behavior:

```rust
// crates/core/src/language/atoms/directive.rs

pub enum DirectiveAtom {}

language_atom! {
    DirectiveAtom {
        name: "directive",
        grammar_rule: Directive,
        ast: ast::Directive,
        lowered: lowered::Directive,
        build_ast: required,
        format: required,
        lower: required,
        check: required,
        lint: no_effect("directives do not currently lint"),
        variables: no_effect("directives do not currently bind variables"),
        plan: no_effect("directives do not currently affect plans"),
        sql: no_effect("directives do not currently affect SQL"),
        metadata: no_effect("directives are not currently public metadata"),
        editor: required,
    }
}

impl BuildsAst<DirectiveAtom> for AstBuilder<'_> {
    fn build_ast(&self, node: NodeRef) -> ast::Directive {
        todo!()
    }
}

impl FormatsAtom<DirectiveAtom> for CstFormatter<'_> {
    fn format_atom(&mut self, node: SyntaxNodeRef) {
        todo!()
    }
}

impl LowersAtom<DirectiveAtom> for Lowerer {
    fn lower_atom(&mut self, directive: &ast::Directive) -> lowered::Directive {
        todo!()
    }
}

impl ChecksAtom<DirectiveAtom> for Checker {
    fn check_atom(&mut self, directive: &ast::Directive, context: CheckContext<'_>) {
        todo!()
    }
}
```

The atom file should not own stage orchestration, source lookup, scoped-program
construction, or adapter protocol conversion.

## Stage Orchestration

Stages remain responsible for traversal and context. Atoms own behavior for one
construct when the stage reaches it.

For example, checking still owns selection traversal:

```rust
fn check_selection_set(&mut self, table: TableId, selections: &[Selection]) {
    for selection in selections {
        self.check_atom::<FieldSelectionAtom>(selection, table);

        for directive in selection.directives() {
            self.check_atom::<DirectiveAtom>(
                directive,
                CheckContext::directive(DirectiveLocation::Selection, table),
            );
        }
    }
}
```

The atom receives context. It does not discover resolution environments,
visible source units, imports, or catalog snapshots by itself.

This preserves the rule that context enters once at the scoped/checking
boundary.

## Context Rules

Atoms may use context supplied by a stage, but they must not create context.

Allowed:

```rust
impl ChecksAtom<DirectiveAtom> for Checker {
    fn check_atom(&mut self, directive: &ast::Directive, context: CheckContext<'_>) {
        context.catalog();
        context.directive_location();
        context.current_table();
    }
}
```

Not allowed:

```rust
impl ChecksAtom<DirectiveAtom> for Checker {
    fn check_atom(&mut self, directive: &ast::Directive, context: CheckContext<'_>) {
        context.resolve_environment_imports();
        context.load_project_config();
        context.query_source_membership();
    }
}
```

Resolution maps, source-unit membership, config, and catalog snapshots remain
inputs to the compiler graph and scoped program, not atom-owned state.

## Diagnostics

Atoms may define stage-specific diagnostic kinds when the stage owns the
diagnostic carrier:

```rust
pub enum DirectiveCheckDiagnostic {
    UnknownDirective { name: String },
    DirectiveNotAllowed { name: String, location: DirectiveLocation },
    MissingDirectiveArgument { name: String, argument: String },
}
```

The diagnostic must still flow through the common diagnostic carrier and
presentation path. Atom diagnostics should not create a second reporting
system.

If a diagnostic is part of a stage enum, adding it should update:

- the stage diagnostic kind;
- `DiagnosticCode`;
- `DsqlDiagnostic` conversion;
- tests for validate and LSP presentation.

Long term, diagnostic code derivation can be improved, but atoms should not
block on that.

## Formatting

Formatting is CST-owned. `FormatsAtom<A>` should receive syntax-node access,
not just AST data.

The formatter must continue to preserve comments, trivia, malformed regions,
and unknown syntax unless it has enough CST structure to rewrite safely.

Formatting may dispatch directly from grammar rules because formatting is the
stage closest to original source text:

```rust
match node.rule() {
    SyntaxRule::Directive => self.format_atom::<DirectiveAtom>(node),
    SyntaxRule::Clause => self.format_atom::<ClauseAtom>(node),
    _ => self.format_internal(node),
}
```

## Lowering

Lowering is context-free. `LowersAtom<A>` may intern names, collect spans,
extract structural facts, and produce lowered source-owned data.

Lowering must not validate catalog existence, fragment visibility, resolution
scope imports, or environment-specific behavior.

For example, directive lowering can intern directive and argument names, but it
must not decide whether the directive is valid for the current table.

## Checking And Linting

Checking validates language semantics that require source structure, scoped
definitions, catalog information, and selected context.

Linting reports advisory diagnostics and may depend on catalog/config inputs.

Atoms may provide separate check and lint coverage:

```rust
impl ChecksAtom<DirectiveAtom> for Checker {}

impl NoLintEffect<DirectiveAtom> for Linter {
    const REASON: &'static str = "directives currently have no lint rules";
}
```

Check and lint orchestration should stay centralized enough that diagnostics
flow through the same sorted carrier.

## Variable Inference

Variable inference is a first-class semantic consumer. It should be covered by
the atom contract because generated input shapes depend on it.

An atom that can contain or imply variables must implement variable coverage:

```rust
impl InfersVariablesAtom<DirectiveAtom> for VariableInference {}
```

An atom that cannot affect variables must say so:

```rust
impl NoVariableEffect<DirectiveAtom> for VariableInference {
    const REASON: &'static str = "directives do not currently accept variable expressions";
}
```

This prevents input-generation drift.

## Planning And SQL

Planning should consume checked or check-compatible semantic data, not raw CST
nodes. If an atom affects execution or result shape, the plan must carry a
semantic effect.

Examples:

- a filter clause affects `SelectionClauses`;
- a directive such as `@include(if: ...)` might affect result shape, SQL, or
  generated input;
- a metadata-only directive may have no plan effect.

SQL generation should only implement an atom if the atom reaches the plan or SQL
model. Otherwise it should declare no effect.

## Generation Metadata And TypeScript

Generation metadata is part of atom coverage whenever an atom changes public
generated behavior.

Rust metadata coverage should account for:

- metadata structs in `crates/metadata`;
- generator population in `crates/generate`;
- schema and TypeScript metadata output;
- TypeScript renderer/runtime behavior when applicable.

The Rust atom contract can prove Rust-side metadata coverage. TypeScript
coverage still needs tests or generated-type checks because it lives outside the
Rust compiler.

An atom that does not affect generated metadata should declare that explicitly.

## Editor Coverage

Editor coverage includes:

- cursor context;
- completion;
- hover;
- definition;
- semantic tokens;
- LSP conversion when new editor token or completion kinds are introduced.

The editor coverage trait can be coarse at first:

```rust
pub trait ProvidesEditorSupport<A: LanguageAtom> {}

pub trait NoEditorEffect<A: LanguageAtom> {
    const REASON: &'static str;
}
```

If editor drift remains common, split it into:

- `ProvidesCursorContext<A>`;
- `ProvidesCompletion<A>`;
- `ProvidesHover<A>`;
- `ProvidesDefinition<A>`;
- `ProvidesSemanticTokens<A>`.

## Compile-Time Guarantees

The atom model should guarantee:

- deleting or renaming an owned grammar rule breaks compilation;
- two atoms cannot own the same grammar rule;
- every generated grammar rule is owned or explicitly internal;
- every atom has required stage coverage;
- every optional stage has either an implementation or no-effect declaration;
- adding an atom without editor/generation/variable decisions fails to compile.

The model cannot guarantee:

- grammar semantics are correct;
- the atom implementation is called from every traversal;
- diagnostics are precise;
- generated TypeScript behavior is correct;
- runtime SQL behavior matches intent.

Those still need tests and focused review.

## Tests

Each atom should have a small coverage test suite matching its declared stages.

For a fully semantic atom:

- parse/AST fixture;
- formatter fixture;
- lowering assertion;
- check diagnostic fixture;
- lint fixture or no-effect assertion;
- variable inference fixture or no-effect assertion;
- plan fixture or no-effect assertion;
- SQL fixture or no-effect assertion;
- generation metadata fixture or no-effect assertion;
- editor tests for completion/hover/definition/semantic tokens if supported.

No-effect declarations should not require behavior tests in every stage, but
they should be visible in the atom declaration so reviews can challenge them.

## Migration Plan

1. Add `crates/core/src/language` with stage traits and a small atom declaration
   macro.
2. Add grammar-rule marker types for the current generated parser rules.
   Handwritten markers are acceptable for the first version; generation can
   follow.
3. Classify current grammar rules as owned or internal.
4. Pilot with `DirectiveAtom` because directives are already parsed and
   formatted but not fully semantic.
5. Before making directives semantic, replace `Selection.directives:
   Vec<NameRef>` with a first-class `Directive` AST type.
6. Add no-effect declarations for stages where directives intentionally do not
   apply yet.
7. Move directive-specific formatter, AST builder, lowering, checking, and
   editor behavior into `language/atoms/directive.rs`.
8. Repeat for fragment spread and field selection if the pilot reduces drift.

## Current Codebase Friction

The model is compatible with the current codebase, but these areas should be
addressed during migration:

- `SyntaxRule` manually mirrors generated parser rules.
- `AstNode` is too coarse for atom-level dispatch.
- several constructs are represented by shared structs plus `kind` enums rather
  than first-class typed nodes;
- directives are currently just names;
- selection traversal is duplicated across check, lint, variables, plan,
  generation, hover, definition, semantic tokens, and completion;
- TypeScript metadata/rendering lives outside the Rust coverage system.

None of these blocks the model. They define the first refactoring targets.

## Open Questions

- Should atom rule markers be generated by the Lelwel build step or maintained
  by a small handwritten bridge checked against `parser::Rule`?
- Should `Selection` become an enum before or after the directive pilot?
- Should editor coverage start coarse or split into cursor/completion/hover/
  definition/tokens immediately?
- Should metadata coverage include a generated TypeScript smoke test as part of
  the atom contract?
- Should diagnostic code mapping be derived from diagnostic kinds before adding
  many atom-specific diagnostics?
