//! Variable entity: every `$name` / `$$name` occurrence as a fact.
//!
//! Variables live inside expression trees structurally (see `expression`),
//! but inference is set-oriented — "which parameters does this query take,
//! at which binding time, with which types" — so each occurrence also
//! becomes its own fact, anchored into the tree by [`ChildOf`].
//!
//! [`ChildOf`]: crate::facts::ChildOf

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::resolution::{ResolvedClause, index_resolved_clauses};
use crate::schema::{AstFacts, dsql_schema};
use bowl::{
    Commands, Component, DerivedFrom, Entity, Query, Registrar, SystemExt, SystemParam, View,
    Where, With,
};

use crate::catalog::{
    Catalog, CatalogSnapshot, DataType, FieldCheckResult, FieldRef, LiteralKind, MAX_SAFE_INTEGER,
    MIN_SAFE_INTEGER, ScalarValidation, TableRef, TableResolution, TypeCapabilities, TypeKey,
    WireEncoding,
};
use crate::entities::clause::{ClauseFact, OrderDirection, OrderTerm};
use crate::entities::definition::{DefDecl, DefKind};
use crate::entities::expression::{
    BinaryOp, ComparisonOp, Expr, Sigil, VariableRef, build_variable_ref,
};
use crate::entities::field_selection::{SelectionTree, TreeViews};
use crate::entities::fragment_spread::{SpreadBindingRef, SpreadDecl, visible_fragments};
use crate::entities::variable_path::{
    InputPathSegment, SelectionPath, VariablePathContext, VariablePathScope,
    predicate_anonymous_key, variable_path,
};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{
    BelongsToFile, ChildOf, DiagnosticCode, DiagnosticFacts, DiagnosticSource, DiagnosticsDemand,
    NodeKey, Severity, Span, VariablesDemand, emit_diagnostic,
};
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{Node, NodeRef, Rule};
use crate::service::completion::{
    CompletionContext, CompletionItem, CompletionKind, CompletionRequest, CompletionSite,
    emit_completion_candidate,
};
use crate::service::hover::{Cursor, HoverEnriched, emit_hover_candidate, priority};
use crate::source::{ResolutionScope, ScopeImports};

/// One variable occurrence, lowered from `value_variable` or
/// `operator_variable`. The inference stage (phase 7) groups these by name
/// and derives the query's parameter set.
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct VariableUse(pub VariableRef);

impl VariableUse {
    pub fn sigil(&self) -> Sigil {
        self.0.sigil
    }
}

/// Owns `value_variable` and `operator_variable`.
pub struct Variable;

impl LanguageEntity for Variable {
    const NAME: &'static str = "variable";

    fn register(reg: &mut Registrar<'_>) {
        // Views lowered facts ambiently: behind the Complete barrier.
        reg.system(infer_variables.run_during(bowl::Phase::Complete));
        reg.system(diagnose_variable_problems);
        // Fully tracked on the duplicate facts: the engine replans the pair
        // after inference commits them at Complete, like variable hover.
        reg.system(diagnose_duplicate_anonymous_bindings);
        // Fully tracked (a per-file bound join, no views), so it needs no
        // phase barrier: pairs replan as bindings commit at Complete.
        reg.system(hover_variables);
        reg.system(complete_definition_inputs.run_during(bowl::Phase::Complete));
    }
}

impl LowerStage for Variable {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        let variable = build_variable_ref(ctx.cst, ctx.source, node);

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        let entity = match ctx.parent {
            Some(parent) => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    VariableUse(variable),
                    ChildOf(parent),
                ))
                .untyped(),
            None => commands
                .insert((
                    DerivedFrom::new(ctx.file),
                    BelongsToFile(ctx.file),
                    key,
                    VariableUse(variable),
                ))
                .untyped(),
        };
        Some(entity)
    }
}

/// Whether a binding surfaces as structured input (`$`, `input.*`) or a
/// top-level parameter (`$$`, `params.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableSource {
    Structured,
    TopLevel,
    Context,
}

/// A compile-time default attached to one inferred public input.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum InputDefault {
    String(String),
    Number(String),
    Boolean(bool),
    Null,
    Collection(Vec<InputDefault>),
    /// The empty identity for a bounded dynamic predicate.
    EmptyObject,
}

/// One definition-header refinement of an inferred public input contract.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct InputRefinement {
    pub source: VariableSource,
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub nullable: bool,
    pub default: Option<InputDefault>,
}

/// Lowers every [`InputRefinement`] directly contained by a definition header.
pub(crate) fn build_input_refinements(
    cst: &crate::grammar::parser::CstData,
    source: &str,
    header: NodeRef,
) -> Vec<InputRefinement> {
    cst.children(header)
        .filter(|child| cst.match_rule(*child, Rule::InputRefinement))
        .filter_map(|node| build_input_refinement(cst, source, node))
        .collect()
}

fn build_input_refinement(
    cst: &crate::grammar::parser::CstData,
    source: &str,
    node: NodeRef,
) -> Option<InputRefinement> {
    let variable = direct_rule(cst, node, Rule::PublicVariable)?;
    let name_span = direct_name(cst, variable)?;
    let source_kind = cst
        .children(variable)
        .find_map(|child| match cst.get(child) {
            Node::Token(Token::Dollar, _) => Some(VariableSource::Structured),
            Node::Token(Token::DollarDollar, _) => Some(VariableSource::TopLevel),
            _ => None,
        })?;
    let default = direct_rule(cst, node, Rule::DefaultValue)
        .and_then(|value| build_input_default(cst, source, value));
    Some(InputRefinement {
        source: source_kind,
        name: text(source, name_span).to_string(),
        name_span,
        span: node_span(cst, node),
        nullable: cst
            .children(node)
            .any(|child| cst.match_token(child, Token::Question).is_some()),
        default,
    })
}

fn build_input_default(
    cst: &crate::grammar::parser::CstData,
    source: &str,
    node: NodeRef,
) -> Option<InputDefault> {
    let value = if cst.match_rule(node, Rule::DefaultValue) {
        cst.children(node).find(|child| {
            matches!(
                cst.get(*child),
                Node::Rule(
                    Rule::Literal | Rule::DefaultCollection | Rule::EmptyObject,
                    _
                )
            )
        })?
    } else {
        node
    };
    match cst.get(value) {
        Node::Rule(Rule::Literal, _) => {
            cst.children(value).find_map(|child| match cst.get(child) {
                Node::Token(Token::String, _) => {
                    let raw = text(source, node_span(cst, child));
                    let inner = raw
                        .strip_prefix('"')
                        .and_then(|raw| raw.strip_suffix('"'))
                        .unwrap_or(raw);
                    Some(InputDefault::String(inner.to_string()))
                }
                Node::Token(Token::Number, _) => Some(InputDefault::Number(
                    text(source, node_span(cst, child)).to_string(),
                )),
                Node::Token(Token::True, _) => Some(InputDefault::Boolean(true)),
                Node::Token(Token::False, _) => Some(InputDefault::Boolean(false)),
                Node::Token(Token::Null, _) => Some(InputDefault::Null),
                _ => None,
            })
        }
        Node::Rule(Rule::DefaultCollection, _) => Some(InputDefault::Collection(
            cst.children(value)
                .filter(|child| cst.match_rule(*child, Rule::DefaultValue))
                .filter_map(|child| build_input_default(cst, source, child))
                .collect(),
        )),
        Node::Rule(Rule::EmptyObject, _) => Some(InputDefault::EmptyObject),
        _ => None,
    }
}

impl From<Sigil> for VariableSource {
    fn from(sigil: Sigil) -> Self {
        match sigil {
            Sigil::Build => Self::Structured,
            Sigil::Query => Self::TopLevel,
            Sigil::Context => Self::Context,
        }
    }
}

/// What a variable occurrence parameterizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableRole {
    WhereValue,
    DynamicPredicate,
    ComparisonOperator,
    SortDirection,
    DynamicOrder,
    Limit,
    Offset,
    FilterAssignment,
}

impl VariableRole {
    /// The artifact label consumed by generated metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            VariableRole::WhereValue => "wherevalue",
            VariableRole::DynamicPredicate => "dynamicpredicate",
            VariableRole::ComparisonOperator => "comparisonoperator",
            VariableRole::SortDirection => "sortdirection",
            VariableRole::DynamicOrder => "dynamicorder",
            VariableRole::Limit => "limit",
            VariableRole::Offset => "offset",
            VariableRole::FilterAssignment => "filterassignment",
        }
    }
}

/// One inferred variable binding: the parameter a query or fragment takes,
/// with its structured path, binding time, and value type. Derived per
/// definition by [`infer_variables`]; the occurrence's [`Span`] rides the
/// same entity as its own component, like diagnostics do.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct VariableBinding {
    pub path: String,
    pub source: VariableSource,
    pub name: Option<String>,
    pub data_type: DataType,
    pub wire: WireEncoding,
    /// Stable provider identity for catalog-backed values.
    pub provider_type: Option<TypeKey>,
    pub collection: bool,
    pub role: VariableRole,
    pub operators: Vec<ComparisonOp>,
    pub enum_values: Vec<String>,
    /// Whether callers must supply this input rather than relying on a default.
    pub required: bool,
    /// Whether callers may explicitly supply `null`.
    pub nullable: bool,
    /// The deterministic replacement used when this input is omitted.
    pub default: Option<InputDefault>,
    /// Whether an independently inferred caller may make this value nullable.
    allows_nullable: bool,
    /// False for a complete contract copied through containment or root lifting.
    refinable: bool,
}

/// All effective bindings for one definition, ordered lexicographically by
/// final input path.
///
/// This aggregate is the tracked input for definition-level services. The
/// individual [`VariableBinding`] facts remain the source for occurrence
/// hover, while metadata consumes this effective contract.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub struct DefinitionVariables {
    pub bindings: Vec<VariableBinding>,
}

/// Validated planner input rewrites keyed by fragment-spread entity. Kept
/// separate from [`DefinitionVariables`] so bindings-only consumers do not
/// track entity-keyed planner state.
#[derive(Component, Debug, Clone, Hash, PartialEq)]
#[component(hash)]
pub(crate) struct DefinitionInputRewrites(
    pub(crate) BTreeMap<Entity, BTreeMap<String, SpreadInputValue>>,
);

/// Immutable definition data copied onto the effective contract fact so
/// planning can run from that tracked fact without stamping the syntax entity
/// or making the contract look like another lowered definition to ambient
/// tree views.
#[derive(Component, Debug, Clone, Hash)]
#[component(hash)]
pub(crate) struct DefinitionVariableOwner {
    pub(crate) definition: Entity,
    pub(crate) declaration: DefDecl,
    pub(crate) scope: ResolutionScope,
}

/// One later anonymous binding whose inferred path duplicates an earlier
/// binding in the same definition. Variable inference owns this semantic
/// fact; diagnostics consume it without repeating the inference walk.
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct DuplicateAnonymousBinding {
    /// Whether the owning definition is a query or fragment.
    pub definition_kind: DefKind,
    /// The owning definition name, for the diagnostic message.
    pub definition_name: String,
    /// The inferred input path shared by both anonymous occurrences.
    pub path: String,
}

/// Infers physical variable occurrences and each definition's effective
/// public contract after recursively applying fragment-spread bindings.
/// Gated on [`VariablesDemand`].
async fn infer_variables(
    _: Query<Entity, With<VariablesDemand>>,
    defs: Query<(Entity, &DefDecl, &NodeKey, &BelongsToFile, &ResolutionScope)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    _index: Query<(Entity, &crate::entities::definition::DefIndex)>,
    views: VariableSemanticViews<'_>,
    mut commands: Commands<(
        dsql_schema::DefinitionVariables,
        dsql_schema::VariableBinding,
        dsql_schema::DuplicateAnonymousBinding,
        dsql_schema::VariableProblem,
    )>,
) {
    let (def_entity, decl, key, file, scope) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();

    let tree = SelectionTree::collect(&views.tree);
    let resolved_clauses =
        index_resolved_clauses(views.resolutions.iter().map(|(_, resolved)| resolved));
    let mut context = ContractContext {
        tree: &tree,
        resolved_clauses: &resolved_clauses,
        catalog: snapshot.catalog(),
        imports: views.imports.item().1,
        stack: Vec::new(),
        completed: HashMap::new(),
    };
    let contract = context.definition_contract(def_entity, decl, &scope.0);
    commands.insert((
        DerivedFrom::many([def_entity, catalog_entity]),
        BelongsToFile(file.0),
        crate::facts::DefKey(def_entity),
        DefinitionVariableOwner {
            definition: def_entity,
            declaration: decl.clone(),
            scope: scope.clone(),
        },
        *key,
        DefinitionVariables {
            bindings: contract.bindings,
        },
        DefinitionInputRewrites(contract.rewrites),
    ));
    for problem in contract.problems {
        commands.insert((
            DerivedFrom::many([def_entity, catalog_entity]),
            BelongsToFile(file.0),
            problem.span,
            problem,
        ));
    }
    let mut anonymous_paths = HashSet::new();
    for (span, binding) in contract.local_bindings {
        if binding.name.is_none() && !anonymous_paths.insert(binding.path.clone()) {
            commands.insert((
                DerivedFrom::many([def_entity, catalog_entity]),
                BelongsToFile(file.0),
                crate::facts::DefKey(def_entity),
                span,
                DuplicateAnonymousBinding {
                    definition_kind: decl.kind,
                    definition_name: decl.name.clone(),
                    path: binding.path.clone(),
                },
            ));
        }
        commands.insert((
            DerivedFrom::many([def_entity, catalog_entity]),
            BelongsToFile(file.0),
            crate::facts::DefKey(def_entity),
            span,
            binding,
        ));
    }
}

#[derive(SystemParam)]
struct VariableSemanticViews<'a> {
    imports: Query<(Entity, &'a ScopeImports)>,
    tree: TreeViews<'a>,
    resolutions: View<'a, (Entity, &'a ResolvedClause)>,
}

struct ContractContext<'a> {
    tree: &'a SelectionTree<'a>,
    resolved_clauses: &'a HashMap<Entity, &'a ResolvedClause>,
    catalog: &'a Catalog,
    imports: &'a ScopeImports,
    stack: Vec<Entity>,
    completed: HashMap<Entity, DefinitionContract>,
}

#[derive(Clone, Default)]
struct DefinitionContract {
    local_bindings: Vec<(Span, VariableBinding)>,
    bindings: Vec<VariableBinding>,
    rewrites: BTreeMap<Entity, BTreeMap<String, SpreadInputValue>>,
    problems: Vec<VariableProblem>,
    cycle_cut: bool,
}

impl ContractContext<'_> {
    fn definition_contract(
        &mut self,
        definition: Entity,
        decl: &DefDecl,
        scope: &str,
    ) -> DefinitionContract {
        if let Some(contract) = self.completed.get(&definition) {
            return contract.clone();
        }
        if self.stack.contains(&definition) {
            return DefinitionContract {
                cycle_cut: true,
                ..DefinitionContract::default()
            };
        }
        self.stack.push(definition);
        let mut inference = Inference {
            tree: self.tree,
            resolved_clauses: self.resolved_clauses,
            catalog: self.catalog,
            bindings: Vec::new(),
        };
        collect_local_definition(&mut inference, definition, decl);
        inference
            .bindings
            .sort_by_key(|(span, _)| (span.start, span.end));
        let local_bindings = inference.bindings.clone();

        let mut problems = Vec::new();
        let mut rewrites = BTreeMap::new();
        let mut cycle_cut = false;
        for (spread_entity, spread, path) in spread_sites(definition, decl, self.tree, self.catalog)
        {
            let candidates = visible_fragments(
                &spread.name,
                scope,
                self.imports,
                self.tree
                    .fragments
                    .iter()
                    .map(|(entity, target_decl, _, target_scope)| {
                        (*entity, *target_decl, *target_scope)
                    }),
            );
            let [(target_entity, target_decl, target_scope)] = candidates.as_slice() else {
                continue;
            };
            let target = self.definition_contract(*target_entity, target_decl, &target_scope.0);
            cycle_cut |= target.cycle_cut;
            rewrites.extend(target.rewrites.clone());
            let mut binding_problems = Vec::new();
            let input_map =
                spread_input_map(spread, &path, &target.bindings, &mut binding_problems);
            inference.bindings.extend(
                input_map
                    .bindings
                    .into_iter()
                    .map(|binding| (spread.span, binding)),
            );
            rewrites.insert(spread_entity, input_map.values);
            problems.extend(binding_problems);
        }
        inference
            .bindings
            .sort_by_key(|(span, _)| (span.start, span.end));
        let refinement_problems = refine_bindings(decl, &mut inference.bindings);
        problems.extend(refinement_problems);
        let mut merge_problems = Vec::new();
        let effective = merge_bindings(&inference.bindings, decl, &mut merge_problems);
        problems.extend(merge_problems);
        self.stack.pop();
        let contract = DefinitionContract {
            local_bindings,
            bindings: effective,
            rewrites,
            problems,
            cycle_cut,
        };
        if !cycle_cut {
            self.completed.insert(definition, contract.clone());
        }
        contract
    }
}

fn collect_local_definition(inference: &mut Inference<'_>, definition: Entity, decl: &DefDecl) {
    if decl.kind == DefKind::Query {
        inference.collect_filter_assignments(definition, &[], &VariablePathScope::operation());
        let roots = inference
            .tree
            .fields_under(definition)
            .map(|(entity, field, _, _)| (*entity, *field))
            .collect::<Vec<_>>();
        for (entity, field) in roots {
            let TableResolution::Found(table) = inference
                .catalog
                .resolve_table_ref_for(TableRef::parse(&field.name))
            else {
                continue;
            };
            let mut path = vec![field.output_key()];
            if field.flattened && field.has_transform() {
                path.push(InputPathSegment::Aggregate.as_ref().to_string());
            }
            inference.collect_selection(
                table.id,
                entity,
                SelectionPath::body(path),
                &VariablePathScope::operation(),
            );
        }
        return;
    }

    if let Some((_, _, target, _)) = inference
        .tree
        .fragments
        .iter()
        .find(|(entity, _, _, _)| *entity == definition)
        && let Some(table) = inference
            .catalog
            .table_ref_for(TableRef::parse(&target.name))
    {
        inference.collect_selection_set(
            table.id,
            definition,
            SelectionPath::fragment_root(),
            &VariablePathScope::fragment(),
        );
    }
}

fn spread_sites<'a>(
    definition: Entity,
    decl: &DefDecl,
    tree: &'a SelectionTree<'a>,
    catalog: &Catalog,
) -> Vec<(Entity, &'a SpreadDecl, SelectionPath)> {
    let mut sites = Vec::new();
    match decl.kind {
        DefKind::Query => {
            for (entity, field, _, _) in tree.fields_under(definition) {
                let TableResolution::Found(table) =
                    catalog.resolve_table_ref_for(TableRef::parse(&field.name))
                else {
                    continue;
                };
                let mut parts = vec![field.output_key()];
                if field.flattened && field.has_transform() {
                    parts.push(InputPathSegment::Aggregate.as_ref().to_string());
                }
                collect_spread_sites(
                    tree,
                    catalog,
                    *entity,
                    table.id,
                    SelectionPath::body(parts),
                    &mut sites,
                );
            }
        }
        DefKind::Fragment => {
            if let Some((_, _, target, _)) = tree
                .fragments
                .iter()
                .find(|(entity, _, _, _)| *entity == definition)
                && let Some(table) = catalog.table_ref_for(TableRef::parse(&target.name))
            {
                collect_spread_sites(
                    tree,
                    catalog,
                    definition,
                    table.id,
                    SelectionPath::fragment_root(),
                    &mut sites,
                );
            }
        }
    }
    sites
}

fn collect_spread_sites<'a>(
    tree: &'a SelectionTree<'a>,
    catalog: &Catalog,
    parent: Entity,
    table: crate::catalog::TableId,
    path: SelectionPath,
    sites: &mut Vec<(Entity, &'a SpreadDecl, SelectionPath)>,
) {
    sites.extend(
        tree.spreads_under(parent)
            .map(|(entity, spread, _)| (*entity, *spread, path.clone())),
    );
    for (entity, field, _, _) in tree.fields_under(parent) {
        let reference = FieldRef {
            target: TableRef::parse(&field.name),
            selector: field.relation_path.as_deref(),
        };
        let FieldCheckResult::Relation(relation) = catalog.check_field_ref(table, reference) else {
            continue;
        };
        let mut child = path.relation_child_path(field.output_key());
        if field.flattened && field.has_transform() {
            child.push(InputPathSegment::Aggregate.as_ref().to_string());
        }
        collect_spread_sites(
            tree,
            catalog,
            *entity,
            relation.table.id,
            SelectionPath::body(child),
            sites,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PublicInputRoot {
    Structured,
    TopLevel,
}

impl PublicInputRoot {
    fn of_path(path: &str) -> Option<Self> {
        if path == "input" || path.starts_with("input.") {
            Some(Self::Structured)
        } else if path == "params" || path.starts_with("params.") {
            Some(Self::TopLevel)
        } else {
            None
        }
    }

    fn of_source(source: VariableSource) -> Option<Self> {
        match source {
            VariableSource::Structured => Some(Self::Structured),
            VariableSource::TopLevel => Some(Self::TopLevel),
            VariableSource::Context => None,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Structured => "input",
            Self::TopLevel => "params",
        }
    }
}

/// One target-fragment path after applying a spread binding list.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum SpreadInputValue {
    Public(String),
    Default(InputDefault),
}

/// Public contracts and per-target path rewrites for one spread instance.
pub(crate) struct SpreadInputMap {
    pub(crate) bindings: Vec<VariableBinding>,
    pub(crate) values: BTreeMap<String, SpreadInputValue>,
}

#[derive(Clone, Debug)]
enum SpreadRootDecision {
    Contained,
    LiftWhole(SpreadBindingRef),
    BindLeaves(BTreeMap<String, SpreadBindingRef>),
    Invalid,
}

/// Applies one spread's containment/lifting rules to its target contract.
pub(crate) fn spread_input_map(
    spread: &SpreadDecl,
    path: &SelectionPath,
    target: &[VariableBinding],
    problems: &mut Vec<VariableProblem>,
) -> SpreadInputMap {
    let mut bindings = Vec::new();
    let mut values = BTreeMap::new();
    for root in [PublicInputRoot::Structured, PublicInputRoot::TopLevel] {
        let target_leaves = target
            .iter()
            .filter(|binding| PublicInputRoot::of_path(&binding.path) == Some(root))
            .collect::<Vec<_>>();
        match decide_spread_root(spread, root, &target_leaves, problems) {
            SpreadRootDecision::Contained => {
                for target in target_leaves {
                    let binding = contained_binding(spread, path, target);
                    values.insert(
                        target.path.clone(),
                        SpreadInputValue::Public(binding.path.clone()),
                    );
                    bindings.push(binding);
                }
            }
            SpreadRootDecision::LiftWhole(source) => {
                for target in target_leaves {
                    let binding = lifted_root_binding(path, target, &source);
                    values.insert(
                        target.path.clone(),
                        SpreadInputValue::Public(binding.path.clone()),
                    );
                    bindings.push(binding);
                }
            }
            SpreadRootDecision::BindLeaves(sources) => {
                for target in target_leaves {
                    if let Some(source) = sources.get(&target.path) {
                        let binding = lifted_leaf_binding(path, target, source);
                        values.insert(
                            target.path.clone(),
                            SpreadInputValue::Public(binding.path.clone()),
                        );
                        bindings.push(binding);
                    } else if let Some(default) = &target.default {
                        values.insert(
                            target.path.clone(),
                            SpreadInputValue::Default(default.clone()),
                        );
                    }
                }
            }
            SpreadRootDecision::Invalid => {}
        }
    }
    for context in target
        .iter()
        .filter(|binding| binding.source == VariableSource::Context)
    {
        bindings.push(context.clone());
        values.insert(
            context.path.clone(),
            SpreadInputValue::Public(context.path.clone()),
        );
    }
    SpreadInputMap { bindings, values }
}

fn decide_spread_root(
    spread: &SpreadDecl,
    root: PublicInputRoot,
    target_leaves: &[&VariableBinding],
    problems: &mut Vec<VariableProblem>,
) -> SpreadRootDecision {
    let root_bindings = spread
        .bindings
        .iter()
        .filter(|binding| PublicInputRoot::of_source(binding.target.source) == Some(root))
        .collect::<Vec<_>>();
    if root_bindings.is_empty() {
        return SpreadRootDecision::Contained;
    }

    let whole = root_bindings
        .iter()
        .filter(|binding| binding.target.name.is_none())
        .collect::<Vec<_>>();
    if whole.len() > 1
        || !whole.is_empty()
            && root_bindings
                .iter()
                .any(|binding| binding.target.name.is_some())
    {
        problems.push(fragment_problem(
            spread.span,
            format!(
                "fragment `{}` mixes or duplicates whole-root and leaf bindings for `{}`",
                spread.name,
                root.prefix()
            ),
        ));
        return SpreadRootDecision::Invalid;
    }
    if let Some(binding) = whole.first() {
        return SpreadRootDecision::LiftWhole(
            binding.source.as_ref().unwrap_or(&binding.target).clone(),
        );
    }

    let mut sources = BTreeMap::new();
    for binding in root_bindings {
        let Some(target_name) = binding.target.name.as_deref() else {
            continue;
        };
        let candidates = target_leaves
            .iter()
            .filter(|target| variable_key(&target.path) == target_name)
            .copied()
            .collect::<Vec<_>>();
        let [target_binding] = candidates.as_slice() else {
            problems.push(fragment_problem(
                binding.span,
                if candidates.is_empty() {
                    format!(
                        "fragment `{}` infers no `{}` input named `{target_name}`",
                        spread.name,
                        root.prefix()
                    )
                } else {
                    format!(
                        "fragment `{}` input `{target_name}` is ambiguous across {}",
                        spread.name,
                        candidates
                            .iter()
                            .map(|candidate| candidate.path.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
            ));
            continue;
        };
        let source = binding.source.as_ref().unwrap_or(&binding.target);
        if sources.contains_key(&target_binding.path) {
            problems.push(fragment_problem(
                binding.span,
                format!(
                    "fragment `{}` input `{target_name}` is bound more than once",
                    spread.name
                ),
            ));
        } else {
            sources.insert(target_binding.path.clone(), source.clone());
        }
    }
    for target in target_leaves {
        if sources.contains_key(&target.path) || target.default.is_some() {
            continue;
        }
        problems.push(fragment_problem(
            spread.span,
            format!(
                "fragment `{}` requires a binding for `{}` in explicitly bound `{}` root",
                spread.name,
                target.path,
                root.prefix()
            ),
        ));
    }
    SpreadRootDecision::BindLeaves(sources)
}

fn contained_binding(
    spread: &SpreadDecl,
    path: &SelectionPath,
    target: &VariableBinding,
) -> VariableBinding {
    let mut parts = vec!["input".to_string()];
    parts.extend(crate::entities::variable_path::fragment_envelope_path(
        path,
        &spread.name,
    ));
    parts.extend(target.path.split('.').map(str::to_string));
    let mut binding = target.clone();
    binding.path = parts.join(".");
    binding.source = VariableSource::Structured;
    binding.refinable = false;
    binding
}

fn lifted_root_binding(
    path: &SelectionPath,
    target: &VariableBinding,
    source: &SpreadBindingRef,
) -> VariableBinding {
    let source_root =
        PublicInputRoot::of_source(source.source).unwrap_or(PublicInputRoot::Structured);
    let mut parts = match source_root {
        PublicInputRoot::TopLevel => vec!["params".to_string()],
        PublicInputRoot::Structured => {
            let mut parts = vec!["input".to_string()];
            parts.extend(spread_site_path(path));
            parts
        }
    };
    if let Some(namespace) = &source.name {
        parts.push(namespace.clone());
    }
    parts.extend(target.path.split('.').skip(1).map(str::to_string));
    let mut binding = target.clone();
    binding.path = parts.join(".");
    binding.source = source.source;
    binding.refinable = false;
    binding
}

fn lifted_leaf_binding(
    path: &SelectionPath,
    target: &VariableBinding,
    source: &SpreadBindingRef,
) -> VariableBinding {
    let inferred = [variable_key(&target.path).to_string()];
    let source_path = variable_path(
        &path.parts,
        VariablePathContext {
            role: target.role,
            inferred_path: &inferred,
            anonymous_key: None,
        },
        &VariablePathScope::operation(),
        match source.source {
            VariableSource::Structured => Sigil::Build,
            VariableSource::TopLevel => Sigil::Query,
            VariableSource::Context => Sigil::Context,
        },
        source.name.as_deref(),
    );
    let mut binding = target.clone();
    binding.path = source_path;
    binding.source = source.source;
    binding.name = source.name.clone();
    binding.required = true;
    binding.nullable = false;
    binding.default = None;
    binding.allows_nullable = target.nullable;
    binding.refinable = true;
    binding
}

fn spread_site_path(path: &SelectionPath) -> Vec<String> {
    let mut parts = path.parts.clone();
    if matches!(
        path.mode,
        crate::entities::variable_path::SelectionPathMode::Body
    ) {
        parts.push(InputPathSegment::Body.as_ref().to_string());
    }
    parts
}

fn variable_key(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

fn fragment_problem(span: Span, message: String) -> VariableProblem {
    VariableProblem {
        span,
        code: DiagnosticCode::InvalidFragmentBinding,
        message,
    }
}

#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct VariableProblem {
    span: Span,
    code: DiagnosticCode,
    message: String,
}

async fn diagnose_variable_problems(
    _: Query<Entity, With<DiagnosticsDemand>>,
    problems: Query<(Entity, &VariableProblem, &Span, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (entity, problem, span, file) = problems.item();
    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(entity),
            file: file.0,
            span: *span,
            severity: Severity::Error,
            source: DiagnosticSource::Generate,
            code: problem.code,
            message: problem.message.clone(),
        },
    );
}

fn refine_bindings(
    decl: &DefDecl,
    bindings: &mut [(Span, VariableBinding)],
) -> Vec<VariableProblem> {
    let mut problems = Vec::new();
    let mut refined = HashSet::new();
    for refinement in &decl.input_refinements {
        let key = (refinement.source, refinement.name.as_str());
        if !refined.insert(key) {
            problems.push(VariableProblem {
                span: refinement.name_span,
                code: DiagnosticCode::InvalidVariableRefinement,
                message: format!(
                    "input `{}` is refined more than once in {} `{}`",
                    refinement.name, decl.kind, decl.name
                ),
            });
            continue;
        }

        let candidate_paths = bindings
            .iter()
            .filter(|(_, binding)| refinement_matches(refinement, binding))
            .map(|(_, binding)| binding.path.as_str())
            .collect::<BTreeSet<_>>();
        if candidate_paths.is_empty() {
            problems.push(VariableProblem {
                span: refinement.name_span,
                code: DiagnosticCode::InvalidVariableRefinement,
                message: format!(
                    "{} `{}` infers no {} input named `{}`",
                    decl.kind,
                    decl.name,
                    source_label(refinement.source),
                    refinement.name
                ),
            });
            continue;
        }
        if refinement.source == VariableSource::Structured && candidate_paths.len() > 1 {
            problems.push(VariableProblem {
                span: refinement.name_span,
                code: DiagnosticCode::InvalidVariableRefinement,
                message: format!(
                    "structured input `{}` is ambiguous; it matches {}",
                    refinement.name,
                    candidate_paths.into_iter().collect::<Vec<_>>().join(", ")
                ),
            });
            continue;
        }

        let mut valid = true;
        for (_, binding) in bindings
            .iter()
            .filter(|(_, binding)| refinement_matches(refinement, binding))
        {
            if let Some(message) = validate_refinement(refinement, binding) {
                problems.push(VariableProblem {
                    span: refinement.span,
                    code: DiagnosticCode::InvalidVariableRefinement,
                    message,
                });
                valid = false;
                break;
            }
        }
        if !valid {
            continue;
        }
        for (_, binding) in bindings
            .iter_mut()
            .filter(|(_, binding)| refinement_matches(refinement, binding))
        {
            binding.nullable = refinement.nullable;
            binding.required = refinement.default.is_none();
            binding.default = refinement.default.clone();
        }
    }
    problems
}

fn refinement_matches(refinement: &InputRefinement, binding: &VariableBinding) -> bool {
    if binding.source != refinement.source {
        return false;
    }
    match refinement.source {
        VariableSource::TopLevel => binding.path == format!("params.{}", refinement.name),
        VariableSource::Structured => {
            binding.refinable
                && (binding.name.as_deref() == Some(refinement.name.as_str())
                    || variable_key(&binding.path) == refinement.name)
        }
        VariableSource::Context => false,
    }
}

fn validate_refinement(refinement: &InputRefinement, binding: &VariableBinding) -> Option<String> {
    if !binding.refinable {
        return Some(format!(
            "input `{}` inherits a fragment root contract and cannot be refined at the caller",
            refinement.name
        ));
    }
    if refinement.nullable && !binding.allows_nullable {
        return Some(format!(
            "nullable caller input `{}` cannot bind a non-null fragment input",
            refinement.name
        ));
    }
    if binding.role == VariableRole::FilterAssignment && refinement.nullable {
        return Some(format!(
            "filter-assignment input `{}` must be a non-null boolean",
            refinement.name
        ));
    }
    if binding.role == VariableRole::ComparisonOperator && refinement.nullable {
        return Some(format!(
            "comparison-operator input `{}` cannot be nullable",
            refinement.name
        ));
    }
    let Some(default) = &refinement.default else {
        return None;
    };
    let capabilities = Catalog::builtin_capabilities(binding.data_type);
    if matches!(default, InputDefault::Null) {
        return (!refinement.nullable).then(|| {
            format!(
                "non-null input `{}` cannot use `null` as its default",
                refinement.name
            )
        });
    }
    if binding.role == VariableRole::DynamicPredicate {
        return (!matches!(default, InputDefault::EmptyObject)).then(|| {
            format!(
                "dynamic predicate `{}` only accepts the empty object default",
                refinement.name
            )
        });
    }
    if binding.role == VariableRole::DynamicOrder {
        return (!matches!(default, InputDefault::Collection(items) if items.is_empty())).then(
            || {
                format!(
                    "dynamic order `{}` only accepts the empty collection default",
                    refinement.name
                )
            },
        );
    }
    if matches!(binding.role, VariableRole::Limit | VariableRole::Offset)
        && let InputDefault::Number(value) = default
        && parse_safe_integer(value).is_none_or(|value| value < 0)
    {
        return Some(format!(
            "{} default for `{}` must be a non-negative integer no greater than {}",
            binding.role.as_str(),
            refinement.name,
            MAX_SAFE_INTEGER,
        ));
    }
    if capabilities.defaults.validation == ScalarValidation::SafeInteger
        && !integer_default_is_safe(default)
    {
        return Some(format!(
            "integer default for `{}` must be between {MIN_SAFE_INTEGER} and {MAX_SAFE_INTEGER}",
            refinement.name,
        ));
    }
    if let InputDefault::Collection(items) = default
        && items.iter().any(|item| matches!(item, InputDefault::Null))
    {
        return Some(format!(
            "collection default for `{}` cannot contain `null` elements",
            refinement.name
        ));
    }
    if input_default_matches(default, binding, &capabilities) {
        None
    } else {
        Some(format!(
            "default for `{}` does not match inferred type {}{}",
            refinement.name,
            binding.data_type.as_str(),
            if binding.collection { "[]" } else { "" }
        ))
    }
}

fn input_default_matches(
    default: &InputDefault,
    binding: &VariableBinding,
    capabilities: &TypeCapabilities,
) -> bool {
    if !binding.enum_values.is_empty() {
        return matches!(
            default,
            InputDefault::String(value) if binding.enum_values.contains(value)
        );
    }
    match default {
        InputDefault::Collection(items) if binding.collection => items.iter().all(|item| {
            !matches!(
                item,
                InputDefault::Null | InputDefault::Collection(_) | InputDefault::EmptyObject
            ) && input_default_scalar_matches(item, capabilities)
        }),
        InputDefault::EmptyObject => false,
        InputDefault::Collection(_) => false,
        scalar if !binding.collection => input_default_scalar_matches(scalar, capabilities),
        _ => false,
    }
}

fn input_default_scalar_matches(default: &InputDefault, capabilities: &TypeCapabilities) -> bool {
    match default {
        InputDefault::String(value) => capabilities.defaults.accepts(LiteralKind::String, value),
        InputDefault::Number(value) => capabilities.defaults.accepts(LiteralKind::Number, value),
        InputDefault::Boolean(value) => capabilities
            .defaults
            .accepts(LiteralKind::Boolean, if *value { "true" } else { "false" }),
        InputDefault::Null => true,
        InputDefault::Collection(_) | InputDefault::EmptyObject => false,
    }
}

fn parse_safe_integer(value: &str) -> Option<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|value| (MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(value))
}

fn integer_default_is_safe(default: &InputDefault) -> bool {
    match default {
        InputDefault::Number(value) => parse_safe_integer(value).is_some(),
        InputDefault::Collection(items) => items.iter().all(integer_default_is_safe),
        _ => true,
    }
}

fn merge_bindings(
    bindings: &[(Span, VariableBinding)],
    decl: &DefDecl,
    problems: &mut Vec<VariableProblem>,
) -> Vec<VariableBinding> {
    let mut merged = BTreeMap::<String, (Span, VariableBinding)>::new();
    for (span, binding) in bindings {
        if let Some((first_span, existing)) = merged.get(&binding.path) {
            if !bindings_compatible(existing, binding) {
                problems.push(VariableProblem {
                    span: *span,
                    code: DiagnosticCode::InvalidVariableRefinement,
                    message: format!(
                        "{} `{}` infers incompatible contracts for `{}` (first used at {}..{})",
                        decl.kind, decl.name, binding.path, first_span.start, first_span.end
                    ),
                });
            }
            continue;
        }
        merged.insert(binding.path.clone(), (*span, binding.clone()));
    }
    let entries = merged.values().collect::<Vec<_>>();
    for (index, (first_span, first)) in entries.iter().enumerate() {
        for (second_span, second) in entries.iter().skip(index + 1) {
            if !path_is_prefix(&first.path, &second.path) {
                continue;
            }
            let (problem_span, origin_span) = if first_span <= second_span {
                (*second_span, *first_span)
            } else {
                (*first_span, *second_span)
            };
            problems.push(VariableProblem {
                span: problem_span,
                code: DiagnosticCode::InvalidFragmentBinding,
                message: format!(
                    "{} `{}` infers `{}` as both a value and an input namespace through `{}` (first used at {}..{})",
                    decl.kind,
                    decl.name,
                    first.path,
                    second.path,
                    origin_span.start,
                    origin_span.end,
                ),
            });
        }
    }
    merged.into_values().map(|(_, binding)| binding).collect()
}

fn path_is_prefix(prefix: &str, path: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|suffix| suffix.starts_with('.'))
}

fn bindings_compatible(left: &VariableBinding, right: &VariableBinding) -> bool {
    left.source == right.source
        && binding_types_compatible(left, right)
        && left.collection == right.collection
        && left.role == right.role
        && left.operators == right.operators
        && left.enum_values == right.enum_values
        && left.required == right.required
        && left.nullable == right.nullable
        && left.default == right.default
}

fn binding_types_compatible(left: &VariableBinding, right: &VariableBinding) -> bool {
    if matches!(left.wire, WireEncoding::TextCast) || matches!(right.wire, WireEncoding::TextCast) {
        return left.wire == right.wire && left.provider_type == right.provider_type;
    }
    left.data_type == right.data_type && left.wire == right.wire
}

fn source_label(source: VariableSource) -> &'static str {
    match source {
        VariableSource::Structured => "structured",
        VariableSource::TopLevel => "top-level",
        VariableSource::Context => "trusted-context",
    }
}

/// Presents duplicate anonymous-binding facts when diagnostics are demanded.
async fn diagnose_duplicate_anonymous_bindings(
    _: Query<Entity, With<DiagnosticsDemand>>,
    duplicates: Query<(Entity, &DuplicateAnonymousBinding, &Span, &BelongsToFile)>,
    mut commands: Commands<(dsql_schema::Diagnostic,)>,
) {
    let (duplicate, binding, span, file) = duplicates.item();
    emit_diagnostic(
        &mut commands,
        DiagnosticFacts {
            derived_from: DerivedFrom::new(duplicate),
            file: file.0,
            span: *span,
            severity: Severity::Error,
            source: DiagnosticSource::Generate,
            code: DiagnosticCode::DuplicateAnonymousVariable,
            message: format!(
                "{} `{}` has multiple anonymous variables for `{}`; name one of them to disambiguate",
                binding.definition_kind, binding.definition_name, binding.path
            ),
        },
    );
}

struct Inference<'a> {
    resolved_clauses: &'a HashMap<Entity, &'a ResolvedClause>,
    tree: &'a SelectionTree<'a>,
    catalog: &'a Catalog,
    bindings: Vec<(Span, VariableBinding)>,
}

impl Inference<'_> {
    /// Clauses of one selection, then its direct field children. Fragment
    /// contracts are composed separately from the memoized spread decisions.
    fn collect_selection(
        &mut self,
        table: crate::catalog::TableId,
        key: Entity,
        path: SelectionPath,
        scope: &VariablePathScope,
    ) {
        let clauses: Vec<_> = self
            .tree
            .clauses_under(key)
            .map(|(entity, clause, _, _)| (*entity, (*clause).clone()))
            .collect();
        for (clause_entity, clause) in clauses {
            let resolved = self.resolved_clauses.get(&clause_entity).copied();
            match clause {
                ClauseFact::FilterAssignment {
                    name, condition, ..
                } => {
                    if let Some(condition) = condition {
                        self.collect_assignment_variables(&path.parts, scope, &name, &condition);
                    }
                }
                ClauseFact::Where { expr } => {
                    if let Some(resolved) = resolved {
                        self.collect_where(&path.parts, scope, &expr, resolved, true);
                    }
                }
                ClauseFact::Limit { expr } => self.push_clause_variable(
                    &path.parts,
                    scope,
                    VariableRole::Limit,
                    InputPathSegment::Limit,
                    &expr,
                ),
                ClauseFact::Offset { expr } => self.push_clause_variable(
                    &path.parts,
                    scope,
                    VariableRole::Offset,
                    InputPathSegment::Offset,
                    &expr,
                ),
                ClauseFact::OrderBy { items } => {
                    for item in items {
                        match item {
                            OrderTerm::Dynamic { variable, .. } => self.push_binding(
                                &path.parts,
                                BindingContext {
                                    role: VariableRole::DynamicOrder,
                                    data_type: DataType::Unknown,
                                    wire: WireEncoding::Unsupported,
                                    provider_type: None,
                                    collection: false,
                                    scope,
                                    inferred_path: &["order".to_string()],
                                    anonymous_key: None,
                                    operators: Vec::new(),
                                    enum_values: Vec::new(),
                                },
                                &variable,
                            ),
                            OrderTerm::Column(item) => {
                                let Some(OrderDirection::Variable(variable)) = &item.direction
                                else {
                                    continue;
                                };
                                let Some(resolved) = resolved else {
                                    continue;
                                };
                                let Some(column_id) = resolved
                                    .order_item_at(item.field_span)
                                    .and_then(|resolved| resolved.column)
                                else {
                                    continue;
                                };
                                let Some(column) = self.catalog.column_by_id(column_id) else {
                                    continue;
                                };
                                let inferred_path = [
                                    column.name.clone(),
                                    InputPathSegment::Direction.as_ref().to_string(),
                                ];
                                self.push_binding(
                                    &path.parts,
                                    BindingContext {
                                        role: VariableRole::SortDirection,
                                        data_type: DataType::Text,
                                        wire: WireEncoding::Text,
                                        provider_type: None,
                                        collection: false,
                                        scope,
                                        inferred_path: &inferred_path,
                                        anonymous_key: None,
                                        operators: Vec::new(),
                                        enum_values: vec!["asc".to_string(), "desc".to_string()],
                                    },
                                    variable,
                                );
                            }
                        }
                    }
                }
            }
        }

        self.collect_selection_set(table, key, path, scope);
    }

    fn collect_filter_assignments(
        &mut self,
        parent: Entity,
        selection_path: &[String],
        scope: &VariablePathScope,
    ) {
        let assignments = self
            .tree
            .clauses_under(parent)
            .filter_map(|(_, clause, _, _)| match clause {
                ClauseFact::FilterAssignment {
                    name,
                    condition: Some(condition),
                    ..
                } => Some((name.clone(), condition.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (name, condition) in assignments {
            self.collect_assignment_variables(selection_path, scope, &name, &condition);
        }
    }

    fn collect_assignment_variables(
        &mut self,
        selection_path: &[String],
        scope: &VariablePathScope,
        filter_name: &str,
        expr: &Expr,
    ) {
        match expr {
            Expr::Variable { variable, .. } => {
                self.push_binding(
                    selection_path,
                    BindingContext {
                        role: VariableRole::FilterAssignment,
                        data_type: DataType::Boolean,
                        wire: WireEncoding::Boolean,
                        provider_type: None,
                        collection: false,
                        scope,
                        inferred_path: &[lower_snake_case(filter_name)],
                        anonymous_key: None,
                        operators: Vec::new(),
                        enum_values: Vec::new(),
                    },
                    variable,
                );
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.collect_assignment_variables(selection_path, scope, filter_name, lhs);
                self.collect_assignment_variables(selection_path, scope, filter_name, rhs);
            }
            Expr::Unary { operand, .. } | Expr::NullTest { operand, .. } => {
                self.collect_assignment_variables(selection_path, scope, filter_name, operand);
            }
            Expr::List { items, .. } => {
                for item in items {
                    self.collect_assignment_variables(selection_path, scope, filter_name, item);
                }
            }
            Expr::Exists {
                filters, predicate, ..
            } => {
                for filter in filters {
                    if let Some(condition) = &filter.condition {
                        self.collect_assignment_variables(
                            selection_path,
                            scope,
                            &filter.name,
                            condition,
                        );
                    }
                }
                if let Some(predicate) = predicate {
                    self.collect_assignment_variables(
                        selection_path,
                        scope,
                        filter_name,
                        predicate,
                    );
                }
            }
            Expr::Literal { .. }
            | Expr::Path { .. }
            | Expr::DynamicPredicate { .. }
            | Expr::PredicateRef { .. }
            | Expr::Aggregate { .. }
            | Expr::Error { .. } => {}
        }
    }

    fn collect_selection_set(
        &mut self,
        table: crate::catalog::TableId,
        parent: Entity,
        path: SelectionPath,
        scope: &VariablePathScope,
    ) {
        let children: Vec<_> = self
            .tree
            .fields_under(parent)
            .map(|(entity, field, _, _)| (*entity, *field))
            .collect();
        for (entity, field) in children {
            let reference = FieldRef {
                target: TableRef::parse(&field.name),
                selector: field.relation_path.as_deref(),
            };
            let FieldCheckResult::Relation(relation) =
                self.catalog.check_field_ref(table, reference)
            else {
                continue;
            };
            let relation_table = relation.table.id;
            let output_name = field
                .alias
                .clone()
                .unwrap_or_else(|| relation.name.to_string());
            let mut child_path = path.relation_child_path(output_name);
            if field.flattened && field.has_transform() {
                child_path.push(InputPathSegment::Aggregate.as_ref().to_string());
            }
            self.collect_selection(
                relation_table,
                entity,
                SelectionPath::body(child_path),
                scope,
            );
        }
    }

    fn collect_where(
        &mut self,
        selection_path: &[String],
        scope: &VariablePathScope,
        expr: &Expr,
        resolved: &ResolvedClause,
        expected_boolean: bool,
    ) {
        match expr {
            Expr::Binary { op, lhs, rhs, .. } => {
                match (lhs.as_ref(), rhs.as_ref()) {
                    (
                        value @ (Expr::Path { .. } | Expr::Aggregate { .. }),
                        Expr::Variable { variable, .. },
                    )
                    | (
                        Expr::Variable { variable, .. },
                        value @ (Expr::Path { .. } | Expr::Aggregate { .. }),
                    ) => {
                        if let Some((data_type, wire, provider_type, field_path)) =
                            self.resolve_predicate_value(value, resolved)
                        {
                            let anonymous_key = variable
                                .name
                                .is_none()
                                .then(|| predicate_anonymous_key(op))
                                .flatten();
                            self.push_binding(
                                selection_path,
                                BindingContext {
                                    role: VariableRole::WhereValue,
                                    data_type,
                                    wire,
                                    provider_type,
                                    collection: matches!(op, BinaryOp::In | BinaryOp::NotIn),
                                    scope,
                                    inferred_path: &field_path,
                                    anonymous_key,
                                    operators: Vec::new(),
                                    enum_values: Vec::new(),
                                },
                                variable,
                            );
                        }
                    }
                    _ => {}
                }

                if let BinaryOp::Variable(operator) = op {
                    let path = match (lhs.as_ref(), rhs.as_ref()) {
                        (value @ (Expr::Path { .. } | Expr::Aggregate { .. }), _)
                        | (_, value @ (Expr::Path { .. } | Expr::Aggregate { .. })) => Some(value),
                        _ => None,
                    };
                    if let Some(path) = path
                        && let Some((data_type, wire, provider_type, field_path)) =
                            self.resolve_predicate_value(path, resolved)
                    {
                        let operators = operator.operators.clone().unwrap_or_default();
                        self.push_binding(
                            selection_path,
                            BindingContext {
                                role: VariableRole::ComparisonOperator,
                                data_type,
                                wire,
                                provider_type,
                                collection: false,
                                scope,
                                inferred_path: &field_path,
                                anonymous_key: None,
                                enum_values: operators
                                    .iter()
                                    .map(|operator| operator.as_str().to_string())
                                    .collect(),
                                operators,
                            },
                            operator,
                        );
                    }
                }

                let child_boolean = matches!(op, BinaryOp::And | BinaryOp::Or);
                self.collect_where(selection_path, scope, lhs, resolved, child_boolean);
                self.collect_where(selection_path, scope, rhs, resolved, child_boolean);
            }
            Expr::Unary { operand, .. } => {
                self.collect_where(selection_path, scope, operand, resolved, true);
            }
            Expr::NullTest { operand, .. } => {
                self.collect_where(selection_path, scope, operand, resolved, false);
            }
            Expr::Exists {
                filters, predicate, ..
            } => {
                for filter in filters {
                    if let Some(condition) = &filter.condition {
                        self.collect_assignment_variables(
                            selection_path,
                            scope,
                            &filter.name,
                            condition,
                        );
                    }
                }
                if let Some(predicate) = predicate {
                    self.collect_where(selection_path, scope, predicate, resolved, true);
                }
            }
            Expr::Variable { variable, .. } if expected_boolean => {
                self.push_binding(
                    selection_path,
                    BindingContext {
                        role: VariableRole::WhereValue,
                        data_type: DataType::Boolean,
                        wire: WireEncoding::Boolean,
                        provider_type: None,
                        collection: false,
                        scope,
                        inferred_path: &["value".to_string()],
                        anonymous_key: None,
                        operators: Vec::new(),
                        enum_values: Vec::new(),
                    },
                    variable,
                );
            }
            Expr::DynamicPredicate { variable, .. } => {
                self.push_binding(
                    selection_path,
                    BindingContext {
                        role: VariableRole::DynamicPredicate,
                        data_type: DataType::Unknown,
                        wire: WireEncoding::Unsupported,
                        provider_type: None,
                        collection: false,
                        scope,
                        inferred_path: &["search".to_string()],
                        anonymous_key: None,
                        operators: Vec::new(),
                        enum_values: Vec::new(),
                    },
                    variable,
                );
            }
            Expr::List { .. }
            | Expr::Aggregate { .. }
            | Expr::Path { .. }
            | Expr::Literal { .. }
            | Expr::Variable { .. }
            | Expr::PredicateRef { .. }
            | Expr::Error { .. } => {}
        }
    }

    /// Terminal column type and display path of a predicate path, read
    /// from the clause resolution facts.
    fn resolve_predicate_path(
        &self,
        path: &Expr,
        resolved_clause: &ResolvedClause,
    ) -> Option<(DataType, WireEncoding, TypeKey, Vec<String>)> {
        let resolved = resolved_clause.path_at(path.span())?;
        let column = resolved.terminal.column()?;
        let data_type = self.catalog.type_for_column(column)?;
        let field_path = resolved.display_path()?.map(str::to_owned).collect();
        Some((
            data_type.data_type,
            data_type.capabilities.wire,
            data_type.key.clone(),
            field_path,
        ))
    }

    fn resolve_predicate_value(
        &self,
        expr: &Expr,
        resolved_clause: &ResolvedClause,
    ) -> Option<(DataType, WireEncoding, Option<TypeKey>, Vec<String>)> {
        match expr {
            Expr::Path { .. } => self.resolve_predicate_path(expr, resolved_clause).map(
                |(data_type, wire, provider_type, path)| {
                    (data_type, wire, Some(provider_type), path)
                },
            ),
            Expr::Aggregate { .. } => {
                let aggregate = resolved_clause.aggregate_at(expr.span())?;
                if !aggregate.is_valid() {
                    return None;
                }
                Some((
                    aggregate.data_type?,
                    Catalog::builtin_capabilities(aggregate.data_type?).wire,
                    None,
                    aggregate.display_path(self.catalog)?,
                ))
            }
            Expr::Binary { .. }
            | Expr::Unary { .. }
            | Expr::NullTest { .. }
            | Expr::List { .. }
            | Expr::Exists { .. }
            | Expr::Literal { .. }
            | Expr::Variable { .. }
            | Expr::DynamicPredicate { .. }
            | Expr::PredicateRef { .. }
            | Expr::Error { .. } => None,
        }
    }

    fn push_clause_variable(
        &mut self,
        selection_path: &[String],
        scope: &VariablePathScope,
        role: VariableRole,
        inferred_key: InputPathSegment,
        expr: &Expr,
    ) {
        let Expr::Variable { variable, .. } = expr else {
            return;
        };
        self.push_binding(
            selection_path,
            BindingContext {
                role,
                data_type: DataType::Int,
                wire: WireEncoding::Integer,
                provider_type: None,
                collection: false,
                scope,
                inferred_path: &[inferred_key.as_ref().to_string()],
                anonymous_key: None,
                operators: Vec::new(),
                enum_values: Vec::new(),
            },
            variable,
        );
    }

    fn push_binding(
        &mut self,
        selection_path: &[String],
        context: BindingContext<'_>,
        variable: &VariableRef,
    ) {
        let name = variable.name.clone();
        let path = variable_path(
            selection_path,
            VariablePathContext {
                role: context.role,
                inferred_path: context.inferred_path,
                anonymous_key: context.anonymous_key,
            },
            context.scope,
            variable.sigil,
            name.as_deref(),
        );
        self.bindings.push((
            variable.span,
            VariableBinding {
                path,
                source: variable.sigil.into(),
                name,
                data_type: context.data_type,
                wire: context.wire,
                provider_type: context.provider_type,
                collection: context.collection,
                role: context.role,
                operators: context.operators,
                enum_values: context.enum_values,
                required: true,
                nullable: false,
                default: None,
                allows_nullable: true,
                refinable: true,
            },
        ));
    }
}

pub(crate) fn lower_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                result.push('_');
            }
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character);
        }
    }
    result
}

struct BindingContext<'a> {
    role: VariableRole,
    data_type: DataType,
    wire: WireEncoding,
    provider_type: Option<TypeKey>,
    collection: bool,
    scope: &'a VariablePathScope,
    inferred_path: &'a [String],
    anonymous_key: Option<&'a str>,
    operators: Vec<ComparisonOp>,
    enum_values: Vec<String>,
}

impl FormatStage for Variable {
    /// Variables are preserved verbatim.
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.write_node_text(node);
    }
}

/// Answers hover on a variable occurrence with its inferred binding: one
/// invocation per (request, binding-in-file) pair via the `BelongsToFile`
/// join, answering when the binding's span holds the cursor. Without
/// `VariablesDemand` there are no bindings, no pairs, and no candidates.
async fn hover_variables(
    query: Query<(Entity, &BelongsToFile, &Cursor), With<HoverEnriched>>,
    bindings: Query<(Entity, &Span, &VariableBinding), Where<bowl::Eq<BelongsToFile>>>,
    mut commands: Commands<(dsql_schema::HoverCandidate,)>,
) {
    let (request, _file, cursor) = query.item();
    let (_, span, binding) = bindings.item();

    if !span.contains(cursor.0) {
        return;
    }

    let binding_time = match binding.source {
        VariableSource::Structured => "build-time",
        VariableSource::TopLevel => "query-time",
        VariableSource::Context => "trusted context",
    };
    let text = format!(
        "{} — `{}`: {} ({binding_time})",
        binding
            .name
            .as_deref()
            .map(|name| format!("`{name}`"))
            .unwrap_or_else(|| "anonymous variable".to_string()),
        binding.path,
        if binding.collection {
            format!("{}[]", binding.data_type.as_str())
        } else {
            binding.data_type.as_str().to_string()
        },
    );

    emit_hover_candidate(&mut commands, request, priority::VARIABLE, text);
}

async fn complete_definition_inputs(
    requests: Query<(Entity, &CompletionContext, &crate::facts::DefKey), With<CompletionRequest>>,
    variables: Query<
        (Entity, &DefinitionVariables, &crate::facts::DefKey),
        Where<bowl::Eq<crate::facts::DefKey>>,
    >,
    mut commands: Commands<(dsql_schema::CompletionCandidate,)>,
) {
    let (request, context, _) = requests.item();
    if context.site != CompletionSite::DefinitionInput {
        return;
    }
    let (_, variables, _) = variables.item();
    let mut seen = HashSet::new();
    let mut items = variables
        .bindings
        .iter()
        .filter(|binding| {
            binding.source != VariableSource::Context
                && context
                    .variable_source
                    .is_none_or(|source| source == binding.source)
        })
        .filter_map(|binding| {
            let name = variable_key(&binding.path);
            seen.insert((binding.source, name)).then(|| CompletionItem {
                label: name.to_string(),
                kind: CompletionKind::Variable,
                detail: Some(format!(
                    "{}{} at {}",
                    binding.data_type.as_str(),
                    if binding.collection { "[]" } else { "" },
                    binding.path
                )),
                documentation: None,
                insert_text: None,
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.label.cmp(&right.label));
    emit_completion_candidate(&mut commands, request, items);
}
