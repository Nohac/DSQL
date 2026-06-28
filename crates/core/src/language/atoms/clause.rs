use crate::language::prelude::*;
use crate::semantic::lower_expr;
use crate::syntax::{
    Clause, LimitClause, OffsetClause, OrderByClause, OrderByItem, SortDirectionExpr, WhereClause,
};

/// Language atom that owns clause wrapper nodes.
pub enum ClauseAtom {}

language_atom! {
    ClauseAtom {
        grammar_rule: Rule::Clause,
        ast: Clause,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("clause validation still runs through centralized semantic selection traversal"),
        lint: no_effect("clause linting is still handled by centralized selection traversal"),
        variables: deferred("clause variable inference still runs through centralized semantic traversal"),
        plan: deferred("clause planning still runs through centralized plan construction"),
        sql: no_effect("clauses are planned before SQL generation"),
        metadata: deferred("clause metadata still runs through centralized metadata generation"),
        editor: deferred("clause editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<ClauseAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> Clause {
        self.clause(node)
            .unwrap_or_else(|| Clause::Where(self.where_clause(node)))
    }
}

impl Formats<ClauseAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.clause(node);
    }
}

impl Lowers<ClauseAtom> for Lowerer {
    fn lower(clause: &Clause, context: &mut LowerContext<'_>) {
        crate::semantic::lower_selection_clauses(std::slice::from_ref(clause), context);
    }
}

deferred_atom_stage_impls!(ClauseAtom {
    check: "clause validation still runs through centralized semantic selection traversal",
    variables: "clause variable inference still runs through centralized semantic traversal",
    plan: "clause planning still runs through centralized plan construction",
    metadata: "clause metadata still runs through centralized metadata generation",
    editor: "clause editor features are not atom-dispatched yet",
});

/// Language atom that owns `where` clauses.
pub enum WhereClauseAtom {}

language_atom! {
    WhereClauseAtom {
        grammar_rule: Rule::WhereClause,
        ast: WhereClause,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("where-clause validation still runs through centralized semantic selection traversal"),
        lint: no_effect("where-clause linting is still handled by centralized selection traversal"),
        variables: deferred("where-clause variable inference still runs through centralized semantic traversal"),
        plan: deferred("where-clause planning still runs through centralized plan construction"),
        sql: no_effect("where clauses are planned before SQL generation"),
        metadata: deferred("where-clause metadata still runs through centralized metadata generation"),
        editor: deferred("where-clause editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<WhereClauseAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> WhereClause {
        self.where_clause(node)
    }
}

impl Formats<WhereClauseAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.write_str("where ");
        if let Some(value) = self.direct_value_rule(node) {
            self.expr(value);
        }
    }
}

impl Lowers<WhereClauseAtom> for Lowerer {
    fn lower(clause: &WhereClause, context: &mut LowerContext<'_>) {
        lower_expr(&clause.predicate, context.interner);
    }
}

deferred_atom_stage_impls!(WhereClauseAtom {
    check: "where-clause validation still runs through centralized semantic selection traversal",
    variables: "where-clause variable inference still runs through centralized semantic traversal",
    plan: "where-clause planning still runs through centralized plan construction",
    metadata: "where-clause metadata still runs through centralized metadata generation",
    editor: "where-clause editor features are not atom-dispatched yet",
});

/// Language atom that owns `order by` clauses.
pub enum OrderByClauseAtom {}

language_atom! {
    OrderByClauseAtom {
        grammar_rule: Rule::OrderByClause,
        ast: OrderByClause,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("order-by validation still runs through centralized semantic selection traversal"),
        lint: no_effect("order-by linting is still handled by centralized selection traversal"),
        variables: deferred("order-by variable inference still runs through centralized semantic traversal"),
        plan: deferred("order-by planning still runs through centralized plan construction"),
        sql: no_effect("order-by clauses are planned before SQL generation"),
        metadata: deferred("order-by metadata still runs through centralized metadata generation"),
        editor: deferred("order-by editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<OrderByClauseAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> OrderByClause {
        self.order_by_clause(node)
    }
}

impl Formats<OrderByClauseAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.write_str("order by ");
        for (idx, item) in self
            .direct_rules(node, SyntaxRule::OrderItem)
            .into_iter()
            .enumerate()
        {
            if idx > 0 {
                self.write_str(", ");
            }
            self.format_child(item);
        }
    }
}

impl Lowers<OrderByClauseAtom> for Lowerer {
    fn lower(clause: &OrderByClause, context: &mut LowerContext<'_>) {
        for item in &clause.items {
            <Lowerer as Lowers<OrderItemAtom>>::lower(item, context);
        }
    }
}

deferred_atom_stage_impls!(OrderByClauseAtom {
    check: "order-by validation still runs through centralized semantic selection traversal",
    variables: "order-by variable inference still runs through centralized semantic traversal",
    plan: "order-by planning still runs through centralized plan construction",
    metadata: "order-by metadata still runs through centralized metadata generation",
    editor: "order-by editor features are not atom-dispatched yet",
});

/// Language atom that owns `limit` clauses.
pub enum LimitClauseAtom {}

language_atom! {
    LimitClauseAtom {
        grammar_rule: Rule::LimitClause,
        ast: LimitClause,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("limit-clause validation still runs through centralized semantic selection traversal"),
        lint: no_effect("limit-clause linting is still handled by centralized selection traversal"),
        variables: deferred("limit-clause variable inference still runs through centralized semantic traversal"),
        plan: deferred("limit-clause planning still runs through centralized plan construction"),
        sql: no_effect("limit clauses are planned before SQL generation"),
        metadata: deferred("limit-clause metadata still runs through centralized metadata generation"),
        editor: deferred("limit-clause editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<LimitClauseAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> LimitClause {
        self.limit_clause(node)
    }
}

impl Formats<LimitClauseAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.write_str("limit ");
        if let Some(value) = self.direct_value_rule(node) {
            self.expr(value);
        }
    }
}

impl Lowers<LimitClauseAtom> for Lowerer {
    fn lower(clause: &LimitClause, context: &mut LowerContext<'_>) {
        lower_expr(&clause.value, context.interner);
    }
}

deferred_atom_stage_impls!(LimitClauseAtom {
    check: "limit-clause validation still runs through centralized semantic selection traversal",
    variables: "limit-clause variable inference still runs through centralized semantic traversal",
    plan: "limit-clause planning still runs through centralized plan construction",
    metadata: "limit-clause metadata still runs through centralized metadata generation",
    editor: "limit-clause editor features are not atom-dispatched yet",
});

/// Language atom that owns `offset` clauses.
pub enum OffsetClauseAtom {}

language_atom! {
    OffsetClauseAtom {
        grammar_rule: Rule::OffsetClause,
        ast: OffsetClause,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("offset-clause validation still runs through centralized semantic selection traversal"),
        lint: no_effect("offset-clause linting is still handled by centralized selection traversal"),
        variables: deferred("offset-clause variable inference still runs through centralized semantic traversal"),
        plan: deferred("offset-clause planning still runs through centralized plan construction"),
        sql: no_effect("offset clauses are planned before SQL generation"),
        metadata: deferred("offset-clause metadata still runs through centralized metadata generation"),
        editor: deferred("offset-clause editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<OffsetClauseAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> OffsetClause {
        self.offset_clause(node)
    }
}

impl Formats<OffsetClauseAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.write_str("offset ");
        if let Some(value) = self.direct_value_rule(node) {
            self.expr(value);
        }
    }
}

impl Lowers<OffsetClauseAtom> for Lowerer {
    fn lower(clause: &OffsetClause, context: &mut LowerContext<'_>) {
        lower_expr(&clause.value, context.interner);
    }
}

deferred_atom_stage_impls!(OffsetClauseAtom {
    check: "offset-clause validation still runs through centralized semantic selection traversal",
    variables: "offset-clause variable inference still runs through centralized semantic traversal",
    plan: "offset-clause planning still runs through centralized plan construction",
    metadata: "offset-clause metadata still runs through centralized metadata generation",
    editor: "offset-clause editor features are not atom-dispatched yet",
});

/// Language atom that owns order-by items.
pub enum OrderItemAtom {}

language_atom! {
    OrderItemAtom {
        grammar_rule: Rule::OrderItem,
        ast: OrderByItem,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("order-item validation still runs through centralized semantic selection traversal"),
        lint: no_effect("order-item linting is still handled by centralized selection traversal"),
        variables: deferred("order-item variable inference still runs through centralized semantic traversal"),
        plan: deferred("order-item planning still runs through centralized plan construction"),
        sql: no_effect("order items are planned before SQL generation"),
        metadata: deferred("order-item metadata still runs through centralized metadata generation"),
        editor: deferred("order-item editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<OrderItemAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> OrderByItem {
        self.order_by_item(node)
    }
}

impl Formats<OrderItemAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.order_item(node);
    }
}

impl Lowers<OrderItemAtom> for Lowerer {
    fn lower(item: &OrderByItem, context: &mut LowerContext<'_>) {
        <Lowerer as Lowers<SortDirectionAtom>>::lower(&item.direction, context);
    }
}

deferred_atom_stage_impls!(OrderItemAtom {
    check: "order-item validation still runs through centralized semantic selection traversal",
    variables: "order-item variable inference still runs through centralized semantic traversal",
    plan: "order-item planning still runs through centralized plan construction",
    metadata: "order-item metadata still runs through centralized metadata generation",
    editor: "order-item editor features are not atom-dispatched yet",
});

/// Language atom that owns sort direction syntax.
pub enum SortDirectionAtom {}

language_atom! {
    SortDirectionAtom {
        grammar_rule: Rule::SortDirection,
        ast: SortDirectionExpr,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("sort-direction validation still runs through centralized semantic selection traversal"),
        lint: no_effect("sort directions do not produce lint diagnostics"),
        variables: deferred("sort-direction variable inference still runs through centralized semantic traversal"),
        plan: deferred("sort-direction planning still runs through centralized plan construction"),
        sql: no_effect("sort directions are planned before SQL generation"),
        metadata: deferred("sort-direction metadata still runs through centralized metadata generation"),
        editor: deferred("sort-direction editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<SortDirectionAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> SortDirectionExpr {
        self.sort_direction(node)
    }
}

impl Formats<SortDirectionAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(&text);
    }
}

impl Lowers<SortDirectionAtom> for Lowerer {
    fn lower(direction: &SortDirectionExpr, context: &mut LowerContext<'_>) {
        if let SortDirectionExpr::Variable(variable) = direction
            && let Some(name) = &variable.name
        {
            context.interner.intern(&name.text);
        }
    }
}

deferred_atom_stage_impls!(SortDirectionAtom {
    check: "sort-direction validation still runs through centralized semantic selection traversal",
    variables: "sort-direction variable inference still runs through centralized semantic traversal",
    plan: "sort-direction planning still runs through centralized plan construction",
    metadata: "sort-direction metadata still runs through centralized metadata generation",
    editor: "sort-direction editor features are not atom-dispatched yet",
});
