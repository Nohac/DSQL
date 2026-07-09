//! Variable entity: every `$name` / `$$name` occurrence as a fact.
//!
//! Variables live inside expression trees structurally (see `expression`),
//! but inference is set-oriented — "which parameters does this query take,
//! at which binding time, with which types" — so each occurrence also
//! becomes its own fact, anchored into the tree by [`ParentKey`].

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Query, SystemExt, Where, With};

use crate::catalog::{
    CatalogSnapshot, DataType, FieldCheckResult, FieldRef, TableRef, TableResolution,
};
use crate::entities::clause::{ClauseFact, OrderDirection};
use crate::entities::definition::{DefDecl, DefKind};
use crate::entities::expression::{
    BinaryOp, ComparisonOp, Expr, Sigil, VariableRef, build_variable_ref,
};
use crate::entities::field_selection::{SelectionTree, TreeViews};
use crate::entities::variable_path::{
    InputPathSegment, SelectionPath, VariablePathContext, VariablePathScope, variable_path,
};
use crate::entity::{
    CompletionStage, FormatStage, HoverStage, LanguageEntity, LowerCtx, LowerStage,
};
use crate::facts::{BelongsToFile, ChildOf, NodeKey, Span, VariablesDemand};
use crate::format::CstFormatter;
use crate::grammar::parser::NodeRef;
use crate::service::hover::{HoverCandidate, HoverEnriched, Position, RequestKey, priority};
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

    async fn register(bowl: &Bowl) {
        // Views lowered facts ambiently: behind the Complete barrier.
        bowl.add_system(infer_variables.run_during(bowl::Phase::Complete))
            .await;
    }
}

impl LowerStage for Variable {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) -> Option<Entity> {
        let variable = build_variable_ref(ctx.cst, ctx.source, node);

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        let entity = commands.insert((
            DerivedFrom::new(ctx.file),
            BelongsToFile(ctx.file),
            key,
            VariableUse(variable),
        ));
        if let Some(parent) = ctx.parent {
            commands.entity(entity).insert(ChildOf(parent));
        }
        Some(entity)
    }
}

/// Whether a binding surfaces as structured input (`$`, `input.*`) or a
/// top-level parameter (`$$`, `params.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableSource {
    Structured,
    TopLevel,
}

impl From<Sigil> for VariableSource {
    fn from(sigil: Sigil) -> Self {
        match sigil {
            Sigil::Build => Self::Structured,
            Sigil::Query => Self::TopLevel,
        }
    }
}

/// What a variable occurrence parameterizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableRole {
    WhereValue,
    ComparisonOperator,
    SortDirection,
    Limit,
    Offset,
}

impl VariableRole {
    /// The artifact label consumed by generated metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            VariableRole::WhereValue => "wherevalue",
            VariableRole::ComparisonOperator => "comparisonoperator",
            VariableRole::SortDirection => "sortdirection",
            VariableRole::Limit => "limit",
            VariableRole::Offset => "offset",
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
    pub role: VariableRole,
    pub operators: Vec<ComparisonOp>,
    pub enum_values: Vec<String>,
}

/// Infers the variable bindings of each definition: queries bind their own
/// clauses (spreads are
/// not expanded — a fragment's parameters belong to the fragment), while
/// fragment bodies do expand nested spreads with an enveloped path scope.
/// Gated on [`VariablesDemand`].
async fn infer_variables(
    _: Query<Entity, With<VariablesDemand>>,
    defs: Query<(Entity, &DefDecl, &BelongsToFile, &ResolutionScope)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    _index: Query<(Entity, &crate::entities::definition::DefIndex)>,
    imports: Query<(Entity, &ScopeImports)>,
    views: TreeViews<'_>,
    mut commands: Commands,
) {
    let (def_entity, decl, file, scope) = defs.item();
    let (catalog_entity, snapshot) = catalog.item();
    let (_, imports) = imports.item();

    let tree = SelectionTree::collect(&views);
    let mut inference = Inference {
        tree: &tree,
        catalog: snapshot.catalog(),
        scope: &scope.0,
        imports,
        bindings: Vec::new(),
    };

    match decl.kind {
        DefKind::Query => {
            let roots: Vec<_> = tree
                .fields_under(def_entity)
                .map(|(entity, field, _, _)| (*entity, *field))
                .collect();
            for (entity, field) in roots {
                let TableResolution::Found(table) = inference
                    .catalog
                    .resolve_table_ref_for(TableRef::parse(&field.name))
                else {
                    continue;
                };
                let table_id = table.id;
                let path = SelectionPath::body(vec![response_key(field)]);
                inference.collect_selection(
                    table_id,
                    table_id,
                    entity,
                    path,
                    &VariablePathScope::operation(),
                    None,
                );
            }
        }
        DefKind::Fragment => {
            let Some((_, _, target, _)) = tree
                .fragments
                .iter()
                .find(|(entity, _, _, _)| *entity == def_entity)
            else {
                return;
            };
            let Some(table) = inference
                .catalog
                .table_ref_for(TableRef::parse(&target.name))
            else {
                return;
            };
            let table_id = table.id;
            inference.collect_selection_set(
                table_id,
                table_id,
                def_entity,
                SelectionPath::fragment_root(),
                &VariablePathScope::fragment(),
                Some(&mut Vec::new()),
            );
        }
    }

    for (span, binding) in inference.bindings {
        commands.insert((
            DerivedFrom::many([def_entity, catalog_entity]),
            BelongsToFile(file.0),
            crate::facts::DefKey(def_entity),
            span,
            binding,
        ));
    }
}

struct Inference<'a> {
    tree: &'a SelectionTree<'a>,
    catalog: &'a crate::catalog::Catalog,
    scope: &'a str,
    imports: &'a ScopeImports,
    bindings: Vec<(Span, VariableBinding)>,
}

impl Inference<'_> {
    /// Clauses of one selection, then its children. `visiting` is `Some`
    /// in fragment mode, where nested spreads expand (with cycle guard);
    /// `None` in query mode, where spreads are skipped entirely.
    fn collect_selection(
        &mut self,
        root_table: crate::catalog::TableId,
        table: crate::catalog::TableId,
        key: Entity,
        path: SelectionPath,
        scope: &VariablePathScope,
        visiting: Option<&mut Vec<String>>,
    ) {
        let clauses: Vec<_> = self
            .tree
            .clauses_under(key)
            .map(|(_, clause, _, _)| (*clause).clone())
            .collect();
        for clause in clauses {
            match clause {
                ClauseFact::Where { expr } => {
                    self.collect_where(root_table, table, &path.parts, scope, &expr);
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
                        let Some(OrderDirection::Variable(variable)) = &item.direction else {
                            continue;
                        };
                        let reference = FieldRef {
                            target: TableRef::parse(&item.field),
                            selector: None,
                        };
                        let FieldCheckResult::Column(column) =
                            self.catalog.check_field_ref(table, reference)
                        else {
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
                                data_type: DataType::Unknown,
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

        self.collect_selection_set(root_table, table, key, path, scope, visiting);
    }

    fn collect_selection_set(
        &mut self,
        root_table: crate::catalog::TableId,
        table: crate::catalog::TableId,
        parent: Entity,
        path: SelectionPath,
        scope: &VariablePathScope,
        mut visiting: Option<&mut Vec<String>>,
    ) {
        if let Some(visiting) = visiting.as_deref_mut() {
            let spreads: Vec<_> = self
                .tree
                .spreads_under(parent)
                .map(|(_, spread, _)| spread.name.clone())
                .collect();
            for name in spreads {
                let Some((fragment_entity, _, _, _)) = self
                    .tree
                    .resolve_fragment(&name, self.scope, self.imports)
                    .copied()
                else {
                    continue;
                };
                if visiting.contains(&name) {
                    continue;
                }
                visiting.push(name.clone());
                let spread_scope = scope.for_fragment_spread(&path, &name);
                self.collect_selection_set(
                    root_table,
                    table,
                    fragment_entity,
                    SelectionPath::fragment_root(),
                    &spread_scope,
                    Some(visiting),
                );
                visiting.pop();
            }
        }

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
            let child_path = path.relation_child_path(output_name);
            self.collect_selection(
                root_table,
                relation_table,
                entity,
                SelectionPath::body(child_path),
                scope,
                visiting.as_deref_mut(),
            );
        }
    }

    fn collect_where(
        &mut self,
        root_table: crate::catalog::TableId,
        table: crate::catalog::TableId,
        selection_path: &[String],
        scope: &VariablePathScope,
        expr: &Expr,
    ) {
        let Expr::Binary { op, lhs, rhs, .. } = expr else {
            return;
        };

        match (lhs.as_ref(), rhs.as_ref()) {
            (path @ Expr::Path { .. }, Expr::Variable { variable, .. })
            | (Expr::Variable { variable, .. }, path @ Expr::Path { .. }) => {
                if let Some((data_type, field_path)) =
                    self.resolve_predicate_path(root_table, table, path)
                {
                    let anonymous_key = (variable.name.is_none()
                        && matches!(op, BinaryOp::Variable(_)))
                    .then_some(InputPathSegment::Value.as_ref());
                    self.push_binding(
                        selection_path,
                        BindingContext {
                            role: VariableRole::WhereValue,
                            data_type,
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
                (path @ Expr::Path { .. }, _) | (_, path @ Expr::Path { .. }) => Some(path),
                _ => None,
            };
            if let Some(path) = path
                && let Some((data_type, field_path)) =
                    self.resolve_predicate_path(root_table, table, path)
            {
                self.push_operator_binding(selection_path, scope, data_type, &field_path, operator);
            }
        }

        self.collect_where(root_table, table, selection_path, scope, lhs);
        self.collect_where(root_table, table, selection_path, scope, rhs);
    }

    fn resolve_predicate_path(
        &self,
        root_table: crate::catalog::TableId,
        table: crate::catalog::TableId,
        path: &Expr,
    ) -> Option<(DataType, Vec<String>)> {
        use crate::entities::expression::PathAnchor;

        let Expr::Path {
            anchor, segments, ..
        } = path
        else {
            return None;
        };
        let mut current_table = match anchor {
            PathAnchor::Current => table,
            PathAnchor::Root => root_table,
            PathAnchor::Parent => return None,
        };
        let (last, relations) = segments.split_last()?;
        let mut field_path = Vec::new();
        for segment in relations {
            let reference = FieldRef {
                target: TableRef::parse(&segment.name),
                selector: segment.relation_path.as_deref(),
            };
            let display = reference.display_text();
            let FieldCheckResult::Relation(relation) =
                self.catalog.check_field_ref(current_table, reference)
            else {
                return None;
            };
            field_path.push(display);
            current_table = relation.table.id;
        }
        let reference = FieldRef {
            target: TableRef::parse(&last.name),
            selector: last.relation_path.as_deref(),
        };
        let display = reference.display_text();
        let FieldCheckResult::Column(column) =
            self.catalog.check_field_ref(current_table, reference)
        else {
            return None;
        };
        field_path.push(display);
        Some((column.data_type, field_path))
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
                scope,
                inferred_path: &[inferred_key.as_ref().to_string()],
                anonymous_key: None,
                operators: Vec::new(),
                enum_values: Vec::new(),
            },
            variable,
        );
    }

    fn push_operator_binding(
        &mut self,
        selection_path: &[String],
        scope: &VariablePathScope,
        data_type: DataType,
        inferred_path: &[String],
        operator: &VariableRef,
    ) {
        let name = operator.name.clone();
        let key = name
            .clone()
            .unwrap_or_else(|| InputPathSegment::Op.as_ref().to_string());
        let allowed = operator.operators.clone().unwrap_or_default();
        let path = variable_path(
            selection_path,
            VariablePathContext {
                role: VariableRole::ComparisonOperator,
                inferred_path,
                anonymous_key: None,
            },
            scope,
            operator.sigil,
            Some(&key),
        );
        self.bindings.push((
            operator.span,
            VariableBinding {
                path,
                source: operator.sigil.into(),
                name,
                data_type,
                role: VariableRole::ComparisonOperator,
                enum_values: allowed.iter().map(|op| op.as_str().to_string()).collect(),
                operators: allowed,
            },
        ));
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
                role: context.role,
                operators: context.operators,
                enum_values: context.enum_values,
            },
        ));
    }
}

struct BindingContext<'a> {
    role: VariableRole,
    data_type: DataType,
    scope: &'a VariablePathScope,
    inferred_path: &'a [String],
    anonymous_key: Option<&'a str>,
    operators: Vec<ComparisonOp>,
    enum_values: Vec<String>,
}

/// Output key of a selection: alias, or the object name of its target.
fn response_key(selection: &crate::entities::field_selection::FieldSel) -> String {
    selection
        .alias
        .clone()
        .unwrap_or_else(|| TableRef::parse(&selection.name).name.to_string())
}

impl FormatStage for Variable {
    /// Variables are preserved verbatim.
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.write_node_text(node);
    }
}

impl HoverStage for Variable {
    async fn register_hover(bowl: &Bowl) {
        // Fully tracked (a per-file bound join, no views), so it needs no
        // phase barrier: pairs replan as bindings commit at Complete.
        bowl.add_system(hover_variables).await;
    }
}

/// Answers hover on a variable occurrence with its inferred binding: one
/// invocation per (request, binding-in-file) pair via the `BelongsToFile`
/// join, answering when the binding's span holds the cursor. Without
/// `VariablesDemand` there are no bindings, no pairs, and no candidates.
async fn hover_variables(
    query: Query<(Entity, &BelongsToFile, &Position), With<HoverEnriched>>,
    bindings: Query<(Entity, &Span, &VariableBinding), Where<bowl::Eq<BelongsToFile>>>,
    mut commands: Commands,
) {
    let (request, _file, position) = query.item();
    let (_, span, binding) = bindings.item();

    if !(span.start <= position.offset && position.offset < span.end) {
        return;
    }

    let binding_time = match binding.source {
        VariableSource::Structured => "build-time",
        VariableSource::TopLevel => "query-time",
    };
    let text = format!(
        "{} — `{}`: {} ({binding_time})",
        binding
            .name
            .as_deref()
            .map(|name| format!("`{name}`"))
            .unwrap_or_else(|| "anonymous variable".to_string()),
        binding.path,
        binding.data_type.as_str(),
    );

    commands.insert((
        DerivedFrom::new(request),
        RequestKey(request),
        HoverCandidate {
            priority: priority::VARIABLE,
            text,
        },
    ));
}

impl CompletionStage for Variable {
    /// Variables are free-form names; nothing to suggest yet.
    async fn register_completions(_bowl: &Bowl) {}
}
