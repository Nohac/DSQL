//! Expression entity: the typed expression tree carried inside clause and
//! directive facts.
//!
//! Expressions do not become bowl entities of their own — an expression is
//! only ever meaningful inside the construct that contains it, so the tree
//! is a plain value built once during lowering. Variables are the
//! exception: inference is set-oriented, so each variable occurrence also
//! becomes a fact (see `variable`).

use crate::schema::AstFacts;
use bowl::{Commands, Entity, Registrar};

use crate::entities::{direct_name, direct_rule, node_span, text};
use crate::entity::{FormatStage, LanguageEntity, LowerCtx, LowerStage};
use crate::facts::Span;
use crate::format::CstFormatter;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{CstData, Node, NodeRef, Rule};

/// A dsql expression, lowered from the CST into a self-contained value.
#[derive(Debug, Clone, Hash, PartialEq)]
pub enum Expr {
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    NullTest {
        operand: Box<Expr>,
        negated: bool,
        span: Span,
    },
    List {
        items: Vec<Expr>,
        span: Span,
    },
    Exists {
        source: ExistsSource,
        predicate: Option<Box<Expr>>,
        span: Span,
    },
    Literal {
        value: LiteralValue,
        span: Span,
    },
    /// A scoped path like `.id`, `..parent.field`, or `~root.field`.
    Path {
        anchor: PathAnchor,
        segments: Vec<PathSegment>,
        span: Span,
    },
    Variable {
        variable: VariableRef,
        span: Span,
    },
    /// A closed scalar aggregate over one relation inside a predicate.
    Aggregate {
        source: Box<Expr>,
        function: String,
        function_span: Span,
        operand: Option<Box<Expr>>,
        span: Span,
    },
    /// A hole left by parse-error recovery; parse diagnostics cover it.
    Error {
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Binary { span, .. }
            | Expr::Unary { span, .. }
            | Expr::NullTest { span, .. }
            | Expr::List { span, .. }
            | Expr::Exists { span, .. }
            | Expr::Literal { span, .. }
            | Expr::Path { span, .. }
            | Expr::Variable { span, .. }
            | Expr::Aggregate { span, .. }
            | Expr::Error { span } => *span,
        }
    }
}

/// Binary operator position: a concrete operator, a boolean connective, or
/// an operator variable choosing among allowed comparisons at bind time.
#[derive(Debug, Clone, Hash, PartialEq)]
pub enum BinaryOp {
    Comparison(ComparisonOp),
    In,
    NotIn,
    And,
    Or,
    Variable(VariableRef),
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub enum ExistsSource {
    Relation(Box<Expr>),
    Table { name: String, span: Span },
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ComparisonOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Like,
}

impl ComparisonOp {
    pub fn as_str(self) -> &'static str {
        match self {
            ComparisonOp::Eq => "==",
            ComparisonOp::Ne => "!=",
            ComparisonOp::Gt => ">",
            ComparisonOp::Ge => ">=",
            ComparisonOp::Lt => "<",
            ComparisonOp::Le => "<=",
            ComparisonOp::Like => "like",
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub enum LiteralValue {
    /// Inner text of the string literal, escapes preserved as written.
    String(String),
    /// Raw number text, preserved exactly as written.
    Number(String),
    Bool(bool),
    Null,
}

/// Where a scoped path starts resolving from.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum PathAnchor {
    /// `.` — the current relation.
    Current,
    /// `..` — the parent relation.
    Parent,
    /// `~` — the root relation of the definition.
    Root,
}

/// One `name` or `name->column` step of a scoped path.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct PathSegment {
    pub name: String,
    pub relation_path: Option<String>,
    pub span: Span,
}

/// One `$name` / `$$name` reference, or an operator variable with its
/// allowed comparison set.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct VariableRef {
    pub sigil: Sigil,
    /// Anonymous variables (`$$` with no name) are allowed syntactically;
    /// the variables stage decides what they mean.
    pub name: Option<String>,
    /// `Some` only for operator variables like `$$op[==, !=]`.
    pub operators: Option<Vec<ComparisonOp>>,
    pub span: Span,
}

/// Owns the expression rules. Expressions lower as part of the clause and
/// directive facts that contain them, so the walk claim is a no-op; the
/// real work is [`build_expr`], called by those entities' lowerings.
pub struct Expression;

impl LanguageEntity for Expression {
    const NAME: &'static str = "expression";

    fn register(_reg: &mut Registrar<'_>) {
        // Expression type checks against the catalog land in phase 6.
    }
}

impl LowerStage for Expression {
    // Consumed by the containing clause/directive lowering via `build_expr`.
    fn lower(
        _ctx: &LowerCtx<'_>,
        _node: NodeRef,
        _commands: &mut Commands<AstFacts>,
    ) -> Option<Entity> {
        None
    }
}

/// First direct child of `node` that starts an expression.
pub(crate) fn expr_child(cst: &CstData, node: NodeRef) -> Option<NodeRef> {
    cst.children(node).find(|child| {
        matches!(
            cst.get(*child),
            Node::Rule(
                Rule::Expr
                    | Rule::BinaryExpr
                    | Rule::UnaryExpr
                    | Rule::NullTestExpr
                    | Rule::ScalarAggregateExpr
                    | Rule::CollectionLiteral
                    | Rule::ExistsExpr
                    | Rule::Literal
                    | Rule::ScopedPath
                    | Rule::ValueVariable,
                _
            )
        )
    })
}

/// Builds the expression tree for an `expr`-shaped CST node (`expr`,
/// `binary_expr`, or any expression alternative directly).
pub(crate) fn build_expr(cst: &CstData, source: &str, node: NodeRef) -> Expr {
    let span = node_span(cst, node);
    match cst.get(node) {
        Node::Rule(Rule::BinaryExpr, _) => build_binary(cst, source, node, span),
        Node::Rule(Rule::UnaryExpr, _) => build_unary(cst, source, node, span),
        Node::Rule(Rule::NullTestExpr, _) => build_null_test(cst, source, node, span),
        Node::Rule(Rule::CollectionLiteral, _) => build_list(cst, source, node, span),
        Node::Rule(Rule::ExistsExpr, _) => build_exists(cst, source, node, span),
        Node::Rule(Rule::ScalarAggregateExpr, _) => build_scalar_aggregate(cst, source, node, span),
        Node::Rule(Rule::Expr, _) => match expr_child(cst, node) {
            // `expr` wraps one alternative, or parenthesizes another expr.
            Some(inner) => build_expr(cst, source, inner),
            None => Expr::Error { span },
        },
        Node::Rule(Rule::Literal, _) => build_literal(cst, source, node, span),
        Node::Rule(Rule::ScopedPath, _) => build_path(cst, source, node, span),
        Node::Rule(Rule::ValueVariable, _) => Expr::Variable {
            variable: build_variable_ref(cst, source, node),
            span,
        },
        _ => Expr::Error { span },
    }
}

fn build_binary(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    let mut operands = cst.children(node).filter(|child| is_expr_node(cst, *child));
    let lhs = operands.next();
    let rhs = operands.next();

    let op = binary_operator(cst, source, node);

    match (lhs, rhs, op) {
        (Some(lhs), Some(rhs), Some(op)) => Expr::Binary {
            op,
            lhs: Box::new(build_expr(cst, source, lhs)),
            rhs: Box::new(build_expr(cst, source, rhs)),
            span,
        },
        // Error recovery produced a partial binary expression; keep the one
        // operand we have so checks can still see into it.
        (Some(only), None, _) => build_expr(cst, source, only),
        _ => Expr::Error { span },
    }
}

fn build_unary(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    match cst.children(node).find(|child| is_expr_node(cst, *child)) {
        Some(operand) => Expr::Unary {
            op: UnaryOp::Not,
            operand: Box::new(build_expr(cst, source, operand)),
            span,
        },
        None => Expr::Error { span },
    }
}

fn build_null_test(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    match cst.children(node).find(|child| is_expr_node(cst, *child)) {
        Some(operand) => Expr::NullTest {
            operand: Box::new(build_expr(cst, source, operand)),
            negated: cst
                .children(node)
                .any(|child| cst.match_token(child, Token::Not).is_some()),
            span,
        },
        None => Expr::Error { span },
    }
}

fn build_list(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    Expr::List {
        items: cst
            .children(node)
            .filter(|child| is_expr_node(cst, *child))
            .map(|item| build_expr(cst, source, item))
            .collect(),
        span,
    }
}

fn build_exists(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    let Some(source_node) = direct_rule(cst, node, Rule::ExistsSource) else {
        return Expr::Error { span };
    };
    let exists_source = if let Some(path) = direct_rule(cst, source_node, Rule::ScopedPath) {
        ExistsSource::Relation(Box::new(build_expr(cst, source, path)))
    } else if let Some(table) = direct_rule(cst, source_node, Rule::QualifiedName) {
        let table_span = node_span(cst, table);
        ExistsSource::Table {
            name: text(source, table_span).to_string(),
            span: table_span,
        }
    } else {
        return Expr::Error { span };
    };
    let predicate = direct_rule(cst, source_node, Rule::WhereClause)
        .and_then(|clause| expr_child(cst, clause))
        .map(|predicate| Box::new(build_expr(cst, source, predicate)));
    Expr::Exists {
        source: exists_source,
        predicate,
        span,
    }
}

fn is_expr_node(cst: &CstData, node: NodeRef) -> bool {
    matches!(
        cst.get(node),
        Node::Rule(
            Rule::Expr
                | Rule::BinaryExpr
                | Rule::UnaryExpr
                | Rule::NullTestExpr
                | Rule::ScalarAggregateExpr
                | Rule::CollectionLiteral
                | Rule::ExistsExpr
                | Rule::Literal
                | Rule::ScopedPath
                | Rule::ValueVariable,
            _
        )
    )
}

fn build_scalar_aggregate(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    let mut paths = cst
        .children(node)
        .filter(|child| cst.match_rule(*child, Rule::ScopedPath));
    let source_path = paths.next();
    let operand = paths.next();
    let function_span = direct_name(cst, node);
    match (source_path, function_span) {
        (Some(source_path), Some(function_span)) => Expr::Aggregate {
            source: Box::new(build_expr(cst, source, source_path)),
            function: text(source, function_span).to_string(),
            function_span,
            operand: operand.map(|operand| Box::new(build_expr(cst, source, operand))),
            span,
        },
        _ => Expr::Error { span },
    }
}

/// Extracts the operator of a `binary_expr`: a `binary_operator` child
/// (comparison or operator variable) or a bare `and`/`or` token.
fn binary_operator(cst: &CstData, source: &str, node: NodeRef) -> Option<BinaryOp> {
    let mut membership_negated = false;
    for child in cst.children(node) {
        match cst.get(child) {
            Node::Rule(Rule::BinaryOperator, _) => {
                if let Some(comparison) = direct_rule(cst, child, Rule::ComparisonOperator) {
                    return comparison_op(cst, comparison).map(BinaryOp::Comparison);
                }
                if let Some(variable) = direct_rule(cst, child, Rule::OperatorVariable) {
                    return Some(BinaryOp::Variable(build_variable_ref(
                        cst, source, variable,
                    )));
                }
            }
            Node::Token(Token::Not, _) => membership_negated = true,
            Node::Token(Token::In, _) => {
                return Some(if membership_negated {
                    BinaryOp::NotIn
                } else {
                    BinaryOp::In
                });
            }
            Node::Token(Token::And, _) => return Some(BinaryOp::And),
            Node::Token(Token::Or, _) => return Some(BinaryOp::Or),
            _ => {}
        }
    }
    None
}

fn comparison_op(cst: &CstData, node: NodeRef) -> Option<ComparisonOp> {
    cst.children(node).find_map(|child| match cst.get(child) {
        Node::Token(Token::Eq, _) => Some(ComparisonOp::Eq),
        Node::Token(Token::Ne, _) => Some(ComparisonOp::Ne),
        Node::Token(Token::Gt, _) => Some(ComparisonOp::Gt),
        Node::Token(Token::Ge, _) => Some(ComparisonOp::Ge),
        Node::Token(Token::Lt, _) => Some(ComparisonOp::Lt),
        Node::Token(Token::Le, _) => Some(ComparisonOp::Le),
        Node::Token(Token::Like, _) => Some(ComparisonOp::Like),
        _ => None,
    })
}

fn build_literal(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    let value = cst.children(node).find_map(|child| match cst.get(child) {
        Node::Token(Token::String, _) => {
            let raw = text(source, node_span(cst, child));
            let inner = raw
                .strip_prefix('"')
                .and_then(|raw| raw.strip_suffix('"'))
                .unwrap_or(raw);
            Some(LiteralValue::String(inner.to_string()))
        }
        Node::Token(Token::Number, _) => Some(LiteralValue::Number(
            text(source, node_span(cst, child)).to_string(),
        )),
        Node::Token(Token::True, _) => Some(LiteralValue::Bool(true)),
        Node::Token(Token::False, _) => Some(LiteralValue::Bool(false)),
        Node::Token(Token::Null, _) => Some(LiteralValue::Null),
        _ => None,
    });
    match value {
        Some(value) => Expr::Literal { value, span },
        None => Expr::Error { span },
    }
}

fn build_path(cst: &CstData, source: &str, node: NodeRef, span: Span) -> Expr {
    let anchor = cst
        .children(node)
        .find_map(|child| match cst.get(child) {
            Node::Token(Token::Dot, _) => Some(PathAnchor::Current),
            Node::Token(Token::DotDot, _) => Some(PathAnchor::Parent),
            Node::Token(Token::Tilde, _) => Some(PathAnchor::Root),
            _ => None,
        })
        .unwrap_or(PathAnchor::Current);

    let segments = cst
        .children(node)
        .filter(|child| cst.match_rule(*child, Rule::ScopedPathSegment))
        .map(|segment| {
            let name_span = direct_rule(cst, segment, Rule::QualifiedName)
                .map(|name| node_span(cst, name))
                .unwrap_or_else(|| node_span(cst, segment));
            // `->column` puts a Name token directly under the segment; the
            // relation name's own tokens are nested inside qualified_name.
            let relation_path =
                direct_name(cst, segment).map(|span| text(source, span).to_string());
            PathSegment {
                name: text(source, name_span).to_string(),
                relation_path,
                span: node_span(cst, segment),
            }
        })
        .collect();

    Expr::Path {
        anchor,
        segments,
        span,
    }
}

/// Builds a [`VariableRef`] from a `value_variable` or `operator_variable`
/// node. Shared with the `variable` entity's fact lowering.
pub(crate) fn build_variable_ref(cst: &CstData, source: &str, node: NodeRef) -> VariableRef {
    let context = cst
        .children(node)
        .any(|child| cst.match_token(child, Token::Colon).is_some());
    let sigil = cst
        .children(node)
        .find_map(|child| match cst.get(child) {
            Node::Token(Token::Dollar, _) => Some(Sigil::Build),
            Node::Token(Token::DollarDollar, _) => Some(Sigil::Query),
            _ => None,
        })
        .map(|sigil| if context { Sigil::Context } else { sigil })
        .unwrap_or(Sigil::Query);

    let name = cst.children(node).find_map(|child| match cst.get(child) {
        Node::Token(Token::Name, _) => Some(text(source, node_span(cst, child)).to_string()),
        _ => None,
    });

    let operators = cst.match_rule(node, Rule::OperatorVariable).then(|| {
        cst.children(node)
            .filter(|child| cst.match_rule(*child, Rule::ComparisonOperator))
            .filter_map(|comparison| comparison_op(cst, comparison))
            .collect()
    });

    VariableRef {
        sigil,
        name,
        operators,
        span: node_span(cst, node),
    }
}

/// Whether a variable binds at build time (`$`) or query time (`$$`).
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sigil {
    Build,
    Query,
    Context,
}

impl Sigil {
    pub fn as_str(self) -> &'static str {
        match self {
            Sigil::Build => "$",
            Sigil::Query => "$$",
            Sigil::Context => "$:",
        }
    }
}

impl std::fmt::Display for Expr {
    /// Compact structural rendering for diagnostics and snapshots.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expr::Binary { op, lhs, rhs, .. } => {
                let op = match op {
                    BinaryOp::Comparison(comparison) => comparison.as_str().to_string(),
                    BinaryOp::In => "in".to_string(),
                    BinaryOp::NotIn => "not in".to_string(),
                    BinaryOp::And => "and".to_string(),
                    BinaryOp::Or => "or".to_string(),
                    BinaryOp::Variable(variable) => render_variable(variable),
                };
                write!(f, "({lhs} {op} {rhs})")
            }
            Expr::Unary { operand, .. } => write!(f, "not {operand}"),
            Expr::NullTest {
                operand, negated, ..
            } => write!(f, "{operand} is {}null", if *negated { "not " } else { "" }),
            Expr::List { items, .. } => {
                let rendered: Vec<String> = items.iter().map(ToString::to_string).collect();
                write!(f, "[{}]", rendered.join(", "))
            }
            Expr::Exists {
                source, predicate, ..
            } => {
                f.write_str("exists ")?;
                match source {
                    ExistsSource::Relation(path) => write!(f, "{path}")?,
                    ExistsSource::Table { name, .. } => f.write_str(name)?,
                }
                if let Some(predicate) = predicate {
                    write!(f, "(where {predicate})")?;
                }
                Ok(())
            }
            Expr::Literal { value, .. } => match value {
                LiteralValue::String(inner) => write!(f, "{inner:?}"),
                LiteralValue::Number(raw) => f.write_str(raw),
                LiteralValue::Bool(value) => write!(f, "{value}"),
                LiteralValue::Null => f.write_str("null"),
            },
            Expr::Path {
                anchor, segments, ..
            } => {
                let anchor = match anchor {
                    PathAnchor::Current => ".",
                    PathAnchor::Parent => "..",
                    PathAnchor::Root => "~",
                };
                f.write_str(anchor)?;
                for (index, segment) in segments.iter().enumerate() {
                    if index > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(&segment.name)?;
                    if let Some(path) = &segment.relation_path {
                        write!(f, "->{path}")?;
                    }
                }
                Ok(())
            }
            Expr::Variable { variable, .. } => f.write_str(&render_variable(variable)),
            Expr::Aggregate {
                source,
                function,
                operand,
                ..
            } => {
                write!(f, "{source} | {function}")?;
                if let Some(operand) = operand {
                    write!(f, " {operand}")?;
                }
                Ok(())
            }
            Expr::Error { .. } => f.write_str("<error>"),
        }
    }
}

fn render_variable(variable: &VariableRef) -> String {
    let mut rendered = variable.sigil.as_str().to_string();
    if let Some(name) = &variable.name {
        rendered.push_str(name);
    }
    if let Some(operators) = &variable.operators {
        let operators: Vec<&str> = operators.iter().map(|op| op.as_str()).collect();
        rendered.push_str(&format!("[{}]", operators.join(", ")));
    }
    rendered
}

impl FormatStage for Expression {
    /// Expressions format through the engine's layout machinery.
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef) {
        formatter.expr(node);
    }
}
