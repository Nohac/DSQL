# Language Atoms

Status: partially implemented architecture.

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
  for example a document, directive, field selection, fragment spread, clause,
  variable, or expression form.
- **Grammar rule**: the parser rule that anchors an atom in source syntax.
- **Owned rule**: a grammar rule claimed by exactly one atom.
- **Delegated rule**: a structural grammar rule routed to the atom that owns the
  surrounding construct behavior.
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

impl DelegatedGrammarRule for grammar_rule::FieldSuffix {
    type Atom = FieldSelectionAtom;
}
```

The grammar bridge should assert that every generated rule is either owned by
an atom or delegated to one. This prevents newly added grammar rules from
drifting without a compiler ownership decision.

## Stage Coverage

Coverage traits should be small and stage-specific:

```rust
pub trait BuildsAst<A: LanguageAtom> {
    type Output;

    fn build(&self, node: NodeRef, context: &mut AstBuildContext<'_>) -> Self::Output;
}

pub trait Formats<A: LanguageAtom> {
    fn format(&mut self, node: SyntaxNodeRef, context: &mut FormatContext<'_>) -> FormatEffect;
}

pub trait Lowers<A: LanguageAtom> {
    fn lower(ast: &A::Ast, interner: &mut Interner, names: &mut NameIndex) -> A::Lowered;
}

pub trait Checks<A: LanguageAtom> {
    type Params<'a>: AtomParam<'a, CheckContext<'a>>;

    fn check(params: Self::Params<'_>);
}
```

Stages that may not apply to all atoms should require either a positive
implementation or a no-effect implementation:

```rust
pub trait Plans<A: LanguageAtom> {}

pub trait NoPlanEffect<A: LanguageAtom> {
    const REASON: &'static str;
}
```

The atom contract should accept either:

```rust
impl Plans<DirectiveAtom> for Planner {}
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
    CstFormatter: Formats<A>,
    Lowerer: Lowers<A>,
    Checker: ChecksCoverage<A>,
    Linter: LintCoverage<A>,
    VariableInference: VariableCoverage<A>,
    Planner: PlanCoverage<A>,
    PostgresSqlGenerator: SqlCoverage<A>,
    MetadataGenerator: MetadataCoverage<A>,
    LanguageService: EditorCoverage<A>,
{
}
```

Coverage aliases can allow either implementation or no-effect declarations:

```rust
pub trait PlanCoverage<A: LanguageAtom> {}

impl<A, T> PlanCoverage<A> for T
where
    A: LanguageAtom,
    T: Plans<A>,
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
- a single atom descriptor for tests and debug output;
- stage provider descriptors used by generic dispatchers.

Generated descriptors are the consumer-facing part of the atom system. Stage
orchestration should ask the atom registry for "the formatter for this rule",
"all context providers", or "the checker for this typed node". It should not
hardcode knowledge that a directive, field selection, or fragment spread owns
the current syntax.

## File Layout

Atoms should be vertical slices:

```text
crates/core/src/language/
  mod.rs
  grammar.rs
  stages.rs
  atoms/
    document.rs
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
    type Output = ast::Directive;

    fn build(&self, node: NodeRef, context: &mut AstBuildContext<'_>) -> ast::Directive {
        todo!()
    }
}

impl Formats<DirectiveAtom> for CstFormatter<'_> {
    fn format(&mut self, node: SyntaxNodeRef, context: &mut FormatContext<'_>) -> FormatEffect {
        todo!()
    }
}

impl Lowers<DirectiveAtom> for Lowerer {
    fn lower(directive: &ast::Directive, interner: &mut Interner, names: &mut NameIndex) -> lowered::Directive {
        todo!()
    }
}

impl Checks<DirectiveAtom> for Checker {
    type Params<'a> = (
        &'a DirectiveRegistry,
        &'a DiagnosticStore<CheckDiagnostic>,
        &'a ast::Directive,
        DirectiveLocation,
    );

    fn check(params: Self::Params<'_>) {
        todo!()
    }
}
```

The atom file should not own stage orchestration, source lookup, scoped-program
construction, or adapter protocol conversion.

`DocumentAtom` is a real atom, not merely an internal traversal detail. It owns
the root source construct and should provide document-level AST building,
formatting, root completions, and any source-wide diagnostics or metadata that
belong to the document syntax itself. It can delegate definition work to
`QueryDefAtom` and `FragmentDefAtom` through dispatcher helpers.

`DocumentAtom` must still not own project orchestration. Source loading,
source-unit membership, scoped-program construction, config resolution, Picante
queries, and LSP protocol conversion remain outside the atom.

## Stage Orchestration

Stages remain responsible for traversal and context. Atoms own behavior for one
construct when the stage reaches it.

The intended consumption model is registry-driven:

1. a stage walks the representation it already owns;
2. the stage extracts a parser rule or typed node key from the current item;
3. the stage asks `LanguageAtoms` for the provider registered for that rule and
   capability;
4. the provider calls the typed atom implementation.

For example, the formatter should not have a handwritten branch saying
"directives format here". It should ask for the formatter registered for the
current `SyntaxRule`:

```rust
if let Some(formatter) = LanguageAtoms::formatter_for_syntax_rule(node.rule()) {
    formatter.format(self, node);
} else {
    self.format_legacy(node);
}
```

Likewise, editor completion should not decide which atom might apply at a
cursor. It should build ranked `LanguageContext` values, loop all registered
completion providers, and let each provider return zero or more completions for
contexts it understands.

Stage consumers must pass enough normalized context for the atom capability to
do its work without rediscovering global state. The context should describe the
local role the atom is playing, not force the atom to query project state or
reconstruct traversal history.

For semantic stages, traversal may start from AST or lowered nodes rather than
CST rules. The same rule applies: the traversal owns context and the registry
selects the construct behavior. A checker might still have to walk selection
sets, but the construct-specific step should sit behind a provider lookup:

```rust
fn check_selection_set(&mut self, table: TableId, selections: &[Selection]) {
    for selection in selections {
        self.check_by_kind(selection.kind(), selection, CheckContext::selection(table));

        for directive in selection.directives() {
            self.check_by_kind(
                directive.kind(),
                directive,
                CheckContext::directive(DirectiveLocation::Selection, table),
            );
        }
    }
}
```

The names above are illustrative. The important property is that callers ask
for a capability by rule or typed construct key, while the registry owns the
mapping from that key to `DirectiveAtom` or any other atom. Explicit
`self.check::<DirectiveAtom>(...)` calls are acceptable during migration only
when the surrounding stage has not yet gained a registry descriptor.

The atom receives context. It does not discover resolution environments,
visible source units, imports, catalog snapshots, or traversal state by itself.

This preserves the rule that context enters once at the scoped/checking
boundary.

For formatting, this means parent formatters can provide layout intent rather
than requiring atom formatters to know absolute indentation depth:

```rust
ctx.format_child(directive, LayoutHint::Inline);
ctx.format_child(selection, LayoutHint::IndentedBlock);
```

The directive atom can then write itself as an inline child, while a selection
atom can format block children using the formatter-provided indentation
operations. Atoms should prefer contextual operations such as `space`,
`newline`, `with_indent`, `format_child`, and `preserve_original` over hardcoded
indent levels or parent-specific traversal assumptions.

## Typed Stage Parameters

Atom stage implementations should declare the narrow parameters they need. The
dispatcher owns the full stage context, extracts the declared parameters, and
only then calls the typed atom implementation. This gives atom impls Bevy-like
ergonomics without exposing a broad project context.

For example:

```rust
pub struct LanguageServiceContext<'a> {
    pub request: LanguageServiceRequest<'a>,
    pub language_context: &'a LanguageContext<'a>,
    pub assets: &'a AssetRegistry,
}

pub trait AtomParam<'a, Ctx>: Sized {
    fn extract(context: &'a Ctx) -> Option<Self>;
}

impl<'a> AtomParam<'a, LanguageServiceContext<'a>> for &'a LanguageContext<'a> {
    fn extract(context: &'a LanguageServiceContext<'a>) -> Option<Self> {
        Some(context.language_context)
    }
}

impl<'a> AtomParam<'a, LanguageServiceContext<'a>> for &'a DirectiveRegistry {
    fn extract(context: &'a LanguageServiceContext<'a>) -> Option<Self> {
        context.assets.get::<DirectiveRegistry>()
    }
}
```

Tuple extraction should be generated with `variadics_please` or an equivalent
small local macro so atom impls can request multiple params without hand-written
arity impls.

The atom implementation then names only the resources it needs:

```rust
impl Completer<DirectiveAtom> for LanguageService {
    type Params<'a> = (&'a LanguageContext<'a>, &'a DirectiveRegistry);

    fn completions((context, registry): Self::Params<'_>) -> Vec<EditorCompletion> {
        directive_completions(context, registry)
    }
}
```

Generated adapter descriptors call extraction before invoking the typed impl:

```rust
fn complete<'a, A>(context: &'a LanguageServiceContext<'a>) -> Vec<EditorCompletion>
where
    A: LanguageAtom,
    LanguageService: Completer<A>,
{
    let Some(params) = <LanguageService as Completer<A>>::Params::extract(context) else {
        return Vec::new();
    };

    <LanguageService as Completer<A>>::completions(params)
}
```

The parameter system should preserve the same consumer contract: stages provide
the current node, typed semantic item, and stage context; atom descriptors
extract what their implementation needs. It should not become a back door for
callers to name a specific atom.

## Asset Registry

Use a general `AssetRegistry` for atom-dispatched stages:

```rust
pub struct AssetRegistry {
    pub project: ProjectAssets,
    pub atom: AtomAssets,
}
```

Project assets are long-lived for a project, context, or revision. Examples:

- `DirectiveRegistry`;
- external directive schemas;
- catalog snapshots;
- scoped fragment maps;
- codegen options.

Atom assets are recreated for each stage pass. Examples:

- `DiagnosticStore<CheckDiagnostic>`;
- temporary validation caches;
- completion scratch;
- per-pass accumulators.

The lifecycle for a stage is:

1. build or update project assets;
2. create fresh atom assets;
3. run stage/atom asset providers;
4. dispatch atom implementations;
5. drain atom assets into the stage return value;
6. drop atom assets.

Normal stage execution should not mutate project assets. Project assets are
prepared before the pass; atom assets are the mutable per-pass surface.

Atoms can provide assets during a preparation phase:

```rust
impl ProvidesAssets<DirectiveAtom, LanguageServiceInputs<'_>> for LanguageService {
    fn provide(assets: &mut AssetRegistry, inputs: &LanguageServiceInputs<'_>) {
        assets.project.insert(DirectiveRegistry::system());
    }
}
```

Atom implementations should request specific assets through typed params rather
than accepting `&AssetRegistry` directly. This keeps dependencies visible in the
impl signature.

## Picante Boundaries

Picante should wrap atom-dispatched stage boundaries rather than being
available inside atom implementations.

Short term, memoization should stay coarse:

```rust
parse_file(source) -> SourceFile
lower_file(source_file) -> LoweredFile
check_file(source_file, asset_fingerprint) -> CheckedFile
```

The tracked query builds or receives stable project assets, creates fresh atom
assets, calls the registry-dispatched pure stage code, drains atom assets into
the explicit return value, and returns that value to Picante.

Atoms should consume materialized assets, not `CompilerDb`, `ProjectHost`,
`SourceDb`, LSP state, or file-system handles. If an atom needs data computed
by Picante, the query layer should compute that data before entering the atom
stage and insert the materialized result into project assets.

Longer term, atom declarations may expose memoization policy:

```rust
language_atom! {
    FieldSelectionAtom {
        lower_memo: per_node,
        check_memo: per_node,
        plan_memo: per_node,
    }
}
```

Generated descriptors can then expose policy generically:

```rust
LowerDescriptor {
    input_kind: LowerInputKind::FieldSelection,
    memo_policy: MemoPolicy::PerNode,
    lower: lower_field_selection_adapter,
}
```

The Picante layer can consume those descriptors without knowing which atom owns
the construct. Project assets used by a memoized atom must contribute stable
fingerprints to the tracked input. The atom still remains a pure stage function.

## Context Rules

Atoms may use context supplied by a stage, but they must not create context.

Allowed:

```rust
impl Checks<DirectiveAtom> for Checker {
    type Params<'a> = (
        &'a DirectiveRegistry,
        &'a DiagnosticStore<CheckDiagnostic>,
        &'a ast::Directive,
        DirectiveLocation,
    );

    fn check((registry, diagnostics, directive, location): Self::Params<'_>) {
        validate_directive(registry, diagnostics, directive, location);
    }
}
```

Not allowed:

```rust
impl Checks<DirectiveAtom> for Checker {
    type Params<'a> = (&'a ProjectHost, &'a ast::Directive);

    fn check((project, directive): Self::Params<'_>) {
        project.resolve_environment_imports();
        project.load_project_config();
        project.query_source_membership();
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

Diagnostics should be emitted through typed atom assets rather than carried in
every return type. For example, checking can install a
`DiagnosticStore<CheckDiagnostic>` in atom assets at the start of the pass:

```rust
assets.atom.insert(DiagnosticStore::<CheckDiagnostic>::new());
```

An atom checker can request that store as a parameter:

```rust
impl Checks<DirectiveAtom> for Checker {
    type Params<'a> = (
        &'a DirectiveRegistry,
        &'a DiagnosticStore<CheckDiagnostic>,
        &'a Directive,
        DirectiveLocation,
    );

    fn check((registry, diagnostics, directive, location): Self::Params<'_>) {
        if let Some(diagnostic) = validate_directive(registry, directive, location) {
            diagnostics.push(diagnostic);
        }
    }
}
```

The owning stage drains diagnostics centrally:

```rust
let diagnostics = assets.atom.take_all::<CheckStage>();
```

`take_all::<Stage>()` should be driven by a stage marker trait that knows which
diagnostic stores belong to the stage and how to sort, deduplicate, and convert
them. This preserves explicit stage outputs while avoiding diagnostic plumbing
in every atom return type.

If a diagnostic is part of a stage enum, adding it should update:

- the stage diagnostic kind;
- `DiagnosticCode`;
- `DsqlDiagnostic` conversion;
- tests for validate and LSP presentation.

Long term, diagnostic code derivation can be improved, but atoms should not
block on that.

## Formatting

Formatting is CST-owned. `Formats<A>` should receive syntax-node access,
not just AST data.

The formatter must continue to preserve comments, trivia, malformed regions,
and unknown syntax unless it has enough CST structure to rewrite safely.

Formatting dispatches from grammar rules because formatting is the stage
closest to original source text:

```rust
if let Some(formatter) = LanguageAtoms::formatter_for_syntax_rule(node.rule()) {
    formatter.format(self, node);
} else {
    self.format_legacy(node);
}
```

The formatter may keep legacy fallback paths during migration, but new atom
formatters should be reached through the registry.

CST traversal should be cooperative. A formatter descriptor should return a
traversal effect so the dispatcher knows whether the atom consumed the subtree:

```rust
pub enum FormatEffect {
    HandledSkipChildren,
    HandledContinueChildren,
    NotHandled,
}
```

Most owning atoms should return `HandledSkipChildren` because they own the full
construct and decide how to handle nested syntax. Wrapper and internal nodes can
continue traversal or remain legacy fallback.

When an atom owns an outer construct but needs nested constructs formatted by
their own atoms, it should call a controlled dispatcher handle supplied by the
stage context:

```rust
ctx.format_child(directive, LayoutHint::Inline);
ctx.format_child(selection_set, LayoutHint::IndentedBlock);
```

Atoms should call stage dispatcher helpers such as `format_child`, not
`LanguageAtoms` directly and not another atom by name. The dispatcher remains
responsible for fallback, traversal effects, ordering, and diagnostics.

## AST Building

AST building dispatches from parser rules, but typed atom impls should return
their natural AST type:

```rust
impl BuildsAst<DocumentAtom> for AstBuilder<'_> {
    type Output = Document;

    fn build(&self, node: NodeRef, ctx: &mut AstBuildContext<'_>) -> Document {
        let definitions = ctx.build_children::<Definition>(node, Rule::Definition);

        Document { definitions }
    }
}

impl BuildsAst<FieldSelectionAtom> for AstBuilder<'_> {
    type Output = FieldSelection;

    fn build(&self, node: NodeRef, ctx: &mut AstBuildContext<'_>) -> FieldSelection {
        let name = ctx.build_child::<RelationRef>(name_node);
        let clauses = ctx.build_children::<Clause>(suffix_node, Rule::Clause);
        let directives = ctx.build_children::<Directive>(suffix_node, Rule::Directive);
        let selections = ctx.build_children::<Selection>(suffix_node, Rule::Selection);

        FieldSelection { name, clauses, directives, selections }
    }
}
```

The descriptor erases outputs at the dispatch boundary:

```rust
pub enum AstBuildOutput {
    Document(Document),
    Definition(Definition),
    Selection(Selection),
    Clause(Clause),
    Expr(Expr),
    Directive(Directive),
    QualifiedName(QualifiedNameRef),
    RelationRef(RelationRef),
}
```

Parents request the output type they need:

```rust
let document = ctx.build::<Document>(NodeRef::ROOT);
let definitions = ctx.build_children::<Definition>(document_node, Rule::Definition);
let selections = ctx.build_children::<Selection>(selection_set, Rule::Selection);
let clauses = ctx.build_children::<Clause>(suffix, Rule::Clause);
```

The erased enum should stay an implementation detail of the dispatcher. Atom
code works with typed outputs through `build_child::<T>` and
`build_children::<T>`.

`IntoAstOutput` can be implemented with a declarative macro rather than a
derive macro:

```rust
impl_into_ast_output!(Document => AstBuildOutput::Document);
impl_into_ast_output!(Directive => AstBuildOutput::Directive);
impl_into_ast_output!(FragmentDef => AstBuildOutput::Definition => Definition::Fragment);
impl_into_ast_output!(FieldSelection => AstBuildOutput::Selection => Selection::Field);
```

This keeps nested output mappings explicit while avoiding repetitive impls.

## Lowering

Lowering is context-free. `Lowers<A>` may intern names, collect spans,
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
impl Checks<DirectiveAtom> for Checker {}

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
impl InfersVariables<DirectiveAtom> for VariableInference {}
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

Editor coverage starts with a generic cursor-context phase plus feature
providers. Context providers classify raw parse/cursor evidence into generic
syntax contexts; feature providers consume those contexts and should not
rediscover the cursor role by reparsing source strings:

```rust
pub trait ProvidesContext<A: LanguageAtom> {
    fn contexts(input: &LanguageContextInput<'_>) -> Vec<LanguageContext<'_>>;
}

pub trait Completer<A: LanguageAtom> {
    type Params<'a>: AtomParam<'a, LanguageServiceContext<'a>>;

    fn completions(params: Self::Params<'_>) -> Vec<EditorCompletion>;
}

pub trait NoEditorEffect<A: LanguageAtom> {
    const REASON: &'static str;
}
```

If editor drift remains common, add feature-specific providers:

- `Completer<A>`;
- `HoverProvider<A>`;
- `DefinitionProvider<A>`;
- `SemanticTokenProvider<A>`.

`DocumentAtom` owns root editor behavior. It should provide root completions
such as `query` and `fragment`, and it should classify source-root cursor
contexts without requiring frontend completion code to special-case the document
root.

## Compile-Time Guarantees

The atom model should guarantee:

- deleting or renaming an owned grammar rule breaks compilation;
- two atoms cannot own the same grammar rule;
- every generated grammar rule is owned or explicitly internal;
- every atom has required stage coverage;
- every optional stage has either an implementation or no-effect declaration;
- adding an atom without editor/generation/variable decisions fails to compile;
- registered stage consumers can discover atom providers without naming the
  atom directly.

The model cannot guarantee:

- grammar semantics are correct;
- every legacy traversal has already been converted to registry dispatch;
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

1. Keep `DirectiveAtom` as the reference slice for grammar classification,
   formatting, AST building, checking, and language-service behavior.
2. Add `AssetRegistry` with project assets and per-pass atom assets.
3. Add typed atom parameter extraction, using generated tuple impls to avoid
   handwritten arity boilerplate.
4. Move directive completion to request `(&LanguageContext, &DirectiveRegistry)`
   and provide the system directive registry as a project asset.
5. Add `DocumentAtom` for root AST building, formatting, and root completions.
6. Add traversal effects and dispatcher handles for CST stages so parent atoms
   can consume subtrees while delegating nested constructs back into the same
   stage pipeline.
7. Add AST build descriptors, `AstBuildOutput`, `IntoAstOutput`, and
   `build_child::<T>` / `build_children::<T>` dispatcher helpers.
8. Split shared AST wrappers where needed, especially selections and clauses,
   so semantic stages can dispatch by typed construct rather than
   `SelectionKind` or broad enum branching.
9. Move lowering, checking, variable inference, planning, and generation onto
   atom descriptors after typed AST shapes make those dispatch keys clear.
10. Keep Picante memoization at per-file/stage boundaries until descriptors can
   expose atom-level memoization policy without teaching the DB layer specific
   atom names.

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
