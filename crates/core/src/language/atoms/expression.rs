use crate::language::prelude::*;
use crate::semantic::lower_expr;
use crate::syntax::{BinaryOp, BinaryOperator, Expr, Literal, OperatorVariable, ValueVariable};
use facet::Facet;

/// Parsed binary expression wrapper.
#[derive(Clone, Debug, PartialEq, Facet)]
pub struct BinaryExpr {
    pub expr: Expr,
}

/// Language atom that owns expression wrapper nodes.
pub enum ExprAtom {}

language_atom! {
    ExprAtom {
        grammar_rule: Rule::Expr,
        ast: Expr,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("expression validation still runs through centralized semantic predicate traversal"),
        lint: no_effect("expression linting is still handled by centralized semantic traversal"),
        variables: deferred("expression variable inference still runs through centralized semantic traversal"),
        plan: deferred("expression planning still runs through centralized plan construction"),
        sql: no_effect("expressions are planned before SQL generation"),
        metadata: deferred("expression metadata still runs through centralized metadata generation"),
        editor: deferred("expression editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<ExprAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> Expr {
        self.expr(node)
    }
}

impl Formats<ExprAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.expr(node);
    }
}

impl Lowers<ExprAtom> for Lowerer {
    fn lower(expr: &Expr, context: &mut LowerContext<'_>) {
        lower_expr(expr, context.interner);
    }
}

deferred_atom_stage_impls!(ExprAtom {
    check: "expression validation still runs through centralized semantic predicate traversal",
    variables: "expression variable inference still runs through centralized semantic traversal",
    plan: "expression planning still runs through centralized plan construction",
    metadata: "expression metadata still runs through centralized metadata generation",
    editor: "expression editor features are not atom-dispatched yet",
});

/// Language atom that owns binary expression nodes.
pub enum BinaryExprAtom {}

language_atom! {
    BinaryExprAtom {
        grammar_rule: Rule::BinaryExpr,
        ast: BinaryExpr,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("binary-expression validation still runs through centralized semantic predicate traversal"),
        lint: no_effect("binary-expression linting is still handled by centralized semantic traversal"),
        variables: deferred("binary-expression variable inference still runs through centralized semantic traversal"),
        plan: deferred("binary-expression planning still runs through centralized plan construction"),
        sql: no_effect("binary expressions are planned before SQL generation"),
        metadata: deferred("binary-expression metadata still runs through centralized metadata generation"),
        editor: deferred("binary-expression editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<BinaryExprAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> BinaryExpr {
        BinaryExpr {
            expr: self.binary_expr(node),
        }
    }
}

impl Formats<BinaryExprAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.binary_expr(node);
    }
}

impl Lowers<BinaryExprAtom> for Lowerer {
    fn lower(expr: &BinaryExpr, context: &mut LowerContext<'_>) {
        lower_expr(&expr.expr, context.interner);
    }
}

deferred_atom_stage_impls!(BinaryExprAtom {
    check: "binary-expression validation still runs through centralized semantic predicate traversal",
    variables: "binary-expression variable inference still runs through centralized semantic traversal",
    plan: "binary-expression planning still runs through centralized plan construction",
    metadata: "binary-expression metadata still runs through centralized metadata generation",
    editor: "binary-expression editor features are not atom-dispatched yet",
});

/// Language atom that owns binary operator nodes.
pub enum BinaryOperatorAtom {}

language_atom! {
    BinaryOperatorAtom {
        grammar_rule: Rule::BinaryOperator,
        ast: BinaryOperator,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("binary-operator validation still runs through centralized semantic predicate traversal"),
        lint: no_effect("binary operators do not produce lint diagnostics"),
        variables: deferred("binary-operator variable inference still runs through centralized semantic traversal"),
        plan: deferred("binary-operator planning still runs through centralized plan construction"),
        sql: no_effect("binary operators are planned before SQL generation"),
        metadata: deferred("binary-operator metadata still runs through centralized metadata generation"),
        editor: deferred("binary-operator editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<BinaryOperatorAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> BinaryOperator {
        self.binary_operator_rule(node)
            .unwrap_or(BinaryOperator::Static(BinaryOp::Eq))
    }
}

impl Formats<BinaryOperatorAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(&text);
    }
}

impl Lowers<BinaryOperatorAtom> for Lowerer {
    fn lower(operator: &BinaryOperator, context: &mut LowerContext<'_>) {
        if let BinaryOperator::Variable(variable) = operator {
            <Lowerer as Lowers<OperatorVariableAtom>>::lower(variable, context);
        }
    }
}

deferred_atom_stage_impls!(BinaryOperatorAtom {
    check: "binary-operator validation still runs through centralized semantic predicate traversal",
    variables: "binary-operator variable inference still runs through centralized semantic traversal",
    plan: "binary-operator planning still runs through centralized plan construction",
    metadata: "binary-operator metadata still runs through centralized metadata generation",
    editor: "binary-operator editor features are not atom-dispatched yet",
});

/// Language atom that owns comparison operator nodes.
pub enum ComparisonOperatorAtom {}

language_atom! {
    ComparisonOperatorAtom {
        grammar_rule: Rule::ComparisonOperator,
        ast: BinaryOp,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("comparison-operator validation still runs through centralized semantic predicate traversal"),
        lint: no_effect("comparison operators do not produce lint diagnostics"),
        variables: deferred("comparison-operator variable inference still runs through centralized semantic traversal"),
        plan: deferred("comparison-operator planning still runs through centralized plan construction"),
        sql: no_effect("comparison operators are planned before SQL generation"),
        metadata: deferred("comparison-operator metadata still runs through centralized metadata generation"),
        editor: deferred("comparison-operator editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<ComparisonOperatorAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> BinaryOp {
        self.static_binary_op(node).unwrap_or(BinaryOp::Eq)
    }
}

impl Formats<ComparisonOperatorAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(&text);
    }
}

impl Lowers<ComparisonOperatorAtom> for Lowerer {
    fn lower(_operator: &BinaryOp, _context: &mut LowerContext<'_>) {}
}

deferred_atom_stage_impls!(ComparisonOperatorAtom {
    check: "comparison-operator validation still runs through centralized semantic predicate traversal",
    variables: "comparison-operator variable inference still runs through centralized semantic traversal",
    plan: "comparison-operator planning still runs through centralized plan construction",
    metadata: "comparison-operator metadata still runs through centralized metadata generation",
    editor: "comparison-operator editor features are not atom-dispatched yet",
});

/// Language atom that owns literal nodes.
pub enum LiteralAtom {}

language_atom! {
    LiteralAtom {
        grammar_rule: Rule::Literal,
        ast: Literal,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("literal validation still runs through centralized semantic predicate traversal"),
        lint: no_effect("literals do not produce lint diagnostics"),
        variables: deferred("literal variable inference still runs through centralized semantic traversal"),
        plan: deferred("literal planning still runs through centralized plan construction"),
        sql: no_effect("literals are planned before SQL generation"),
        metadata: deferred("literal metadata still runs through centralized metadata generation"),
        editor: deferred("literal editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<LiteralAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> Literal {
        self.literal_value(node)
    }
}

impl Formats<LiteralAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.literal(node);
    }
}

impl Lowers<LiteralAtom> for Lowerer {
    fn lower(_literal: &Literal, _context: &mut LowerContext<'_>) {}
}

deferred_atom_stage_impls!(LiteralAtom {
    check: "literal validation still runs through centralized semantic predicate traversal",
    variables: "literal variable inference still runs through centralized semantic traversal",
    plan: "literal planning still runs through centralized plan construction",
    metadata: "literal metadata still runs through centralized metadata generation",
    editor: "literal editor features are not atom-dispatched yet",
});

/// Language atom that owns value-variable nodes.
pub enum ValueVariableAtom {}

language_atom! {
    ValueVariableAtom {
        grammar_rule: Rule::ValueVariable,
        ast: ValueVariable,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("value-variable validation still runs through centralized variable inference"),
        lint: no_effect("value variables are validated by centralized variable inference"),
        variables: deferred("value-variable inference still runs through centralized variable inference"),
        plan: deferred("value-variable planning still runs through centralized plan construction"),
        sql: no_effect("value variables are planned before SQL generation"),
        metadata: deferred("value-variable metadata still runs through centralized metadata generation"),
        editor: deferred("value-variable editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<ValueVariableAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> ValueVariable {
        self.value_variable(node)
    }
}

impl Formats<ValueVariableAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        self.value_variable(node);
    }
}

impl Lowers<ValueVariableAtom> for Lowerer {
    fn lower(variable: &ValueVariable, context: &mut LowerContext<'_>) {
        if let Some(name) = &variable.name {
            context.interner.intern(&name.text);
        }
    }
}

deferred_atom_stage_impls!(ValueVariableAtom {
    check: "value-variable validation still runs through centralized variable inference",
    variables: "value-variable inference still runs through centralized variable inference",
    plan: "value-variable planning still runs through centralized plan construction",
    metadata: "value-variable metadata still runs through centralized metadata generation",
    editor: "value-variable editor features are not atom-dispatched yet",
});

/// Language atom that owns operator-variable nodes.
pub enum OperatorVariableAtom {}

language_atom! {
    OperatorVariableAtom {
        grammar_rule: Rule::OperatorVariable,
        ast: OperatorVariable,
        lowered: (),
        build_ast: required,
        format: required,
        lower: required,
        check: deferred("operator-variable validation still runs through centralized variable inference"),
        lint: no_effect("operator variables are validated by centralized variable inference"),
        variables: deferred("operator-variable inference still runs through centralized variable inference"),
        plan: deferred("operator-variable planning still runs through centralized plan construction"),
        sql: no_effect("operator variables are planned before SQL generation"),
        metadata: deferred("operator-variable metadata still runs through centralized metadata generation"),
        editor: deferred("operator-variable editor features are not atom-dispatched yet"),
    }
}

impl BuildsAst<OperatorVariableAtom> for AstBuilder<'_> {
    fn build(&self, node: NodeRef) -> OperatorVariable {
        self.operator_variable(node)
    }
}

impl Formats<OperatorVariableAtom> for CstFormatter<'_> {
    fn format(&mut self, node: usize) {
        let text = self.node_text(node);
        self.write_str(&text);
    }
}

impl Lowers<OperatorVariableAtom> for Lowerer {
    fn lower(variable: &OperatorVariable, context: &mut LowerContext<'_>) {
        if let Some(name) = &variable.name {
            context.interner.intern(&name.text);
        }
    }
}

deferred_atom_stage_impls!(OperatorVariableAtom {
    check: "operator-variable validation still runs through centralized variable inference",
    variables: "operator-variable inference still runs through centralized variable inference",
    plan: "operator-variable planning still runs through centralized plan construction",
    metadata: "operator-variable metadata still runs through centralized metadata generation",
    editor: "operator-variable editor features are not atom-dispatched yet",
});
