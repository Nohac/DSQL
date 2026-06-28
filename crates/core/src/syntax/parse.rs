use super::ast::*;
use super::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSource, Severity};
use super::grammar::lexer::Token;
use super::grammar::parser::{Cst, Node, NodeRef, Parser, Rule};
use super::{Directive, Document, SourceFile, SourceSnapshot, TextRange};
use crate::diagnostics::{
    CompilerDiagnostic, CompilerDiagnosticSource, extend_compiler_diagnostics,
};
use crate::language::grammar::{AstNode, LanguageAtoms};
use facet::Facet;

#[derive(Clone, Debug)]
pub struct ParseResult {
    pub source: SourceSnapshot,
    pub tree: SyntaxTree,
    pub source_file: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

impl CompilerDiagnosticSource for ParseResult {
    fn extend_compiler_diagnostics(&self, diagnostics: &mut Vec<CompilerDiagnostic>) {
        extend_compiler_diagnostics(diagnostics, self.diagnostics.iter().cloned());
    }
}

#[derive(Clone, Debug, Facet)]
pub struct SyntaxTree {
    pub nodes: Vec<SyntaxNode>,
    pub debug: String,
}

#[derive(Clone, Debug, Facet)]
pub struct SyntaxNode {
    pub cst_kind: CstKind,
    pub range: TextRange,
    pub children: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum CstKind {
    Rule(SyntaxRule),
    Token(SyntaxToken),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum SyntaxRule {
    BinaryExpr,
    BinaryOperator,
    Clause,
    ClauseList,
    ComparisonOperator,
    Definition,
    Directive,
    DirectiveArgument,
    DirectiveMember,
    DirectiveName,
    DirectiveNamespace,
    Document,
    Error,
    Expr,
    FieldSelection,
    FieldSelectionTail,
    FieldSuffix,
    FragmentSpread,
    FragmentDef,
    Literal,
    LimitClause,
    OffsetClause,
    OperatorVariable,
    OrderByClause,
    OrderItem,
    QueryDef,
    QualifiedName,
    RelationRef,
    ScopedPath,
    ScopedPathSegment,
    Selection,
    SelectionSet,
    SortDirection,
    ValueVariable,
    WhereClause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum SyntaxToken {
    Query,
    Fragment,
    On,
    Where,
    Order,
    By,
    Limit,
    Offset,
    Asc,
    Desc,
    True,
    False,
    Null,
    Like,
    And,
    Or,
    LBrace,
    RBrace,
    LPar,
    RPar,
    LBracket,
    RBracket,
    ColonColon,
    Arrow,
    Colon,
    At,
    Comma,
    Ellipsis,
    DotDot,
    Dot,
    Tilde,
    DollarDollar,
    Dollar,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Name,
    String,
    Number,
    Whitespace,
    Comment,
    Error,
    Eof,
}

pub fn parse_source(source: SourceSnapshot) -> ParseResult {
    let (tree, source_file, diagnostics) = {
        let source_text = source.source_view();
        let mut lelwel_diagnostics = Vec::new();
        let cst = Parser::new(source_text, &mut lelwel_diagnostics).parse(&mut lelwel_diagnostics);
        let diagnostics = lelwel_diagnostics
            .into_iter()
            .map(convert_diagnostic)
            .collect::<Vec<_>>();
        let document = AstBuilder::new(&cst).document();
        let tree = build_syntax_tree(&cst);
        (tree, SourceFile::new(document), diagnostics)
    };

    ParseResult {
        source,
        tree,
        source_file,
        diagnostics,
    }
}

pub fn expected_tokens_at(source: &SourceSnapshot, byte: usize) -> Vec<SyntaxToken> {
    let source_text = source.source_view();
    if byte > source_text.len() || !source_text.is_char_boundary(byte) {
        return Vec::new();
    }

    let mut completion_text = String::with_capacity(source_text.len() + 1);
    completion_text.push_str(&source_text[..byte]);
    completion_text.push('@');
    completion_text.push_str(&source_text[byte..]);

    let mut diagnostics = Vec::new();
    let cst = Parser::new(&completion_text, &mut diagnostics).parse(&mut diagnostics);
    let mut tokens = cst
        .expected_tokens()
        .iter()
        .filter(|expected| expected.span.start == byte)
        .map(|expected| map_token(expected.token))
        .collect::<Vec<_>>();
    tokens.sort_by_key(|token| *token as u8);
    tokens.dedup();
    tokens
}

impl SyntaxTree {
    pub fn token_nodes(&self) -> impl Iterator<Item = &SyntaxNode> {
        self.nodes
            .iter()
            .filter(|node| matches!(node.cst_kind, CstKind::Token(_)))
    }

    pub fn significant_token_nodes_before(&self, byte: usize) -> impl Iterator<Item = &SyntaxNode> {
        self.token_nodes().filter(move |node| {
            (node.range.start as usize) < byte
                && !matches!(
                    node.cst_kind,
                    CstKind::Token(
                        SyntaxToken::Whitespace | SyntaxToken::Comment | SyntaxToken::Eof
                    )
                )
        })
    }
}

fn convert_diagnostic(diagnostic: super::grammar::parser::Diagnostic) -> Diagnostic {
    let range = diagnostic
        .labels
        .first()
        .map_or(TextRange::default(), |label| {
            TextRange::new(label.range.start, label.range.end)
        });
    Diagnostic {
        range,
        severity: match diagnostic.severity {
            codespan_reporting::diagnostic::Severity::Error
            | codespan_reporting::diagnostic::Severity::Bug => Severity::Error,
            codespan_reporting::diagnostic::Severity::Warning => Severity::Warning,
            codespan_reporting::diagnostic::Severity::Note
            | codespan_reporting::diagnostic::Severity::Help => Severity::Info,
        },
        code: DiagnosticCode::UnexpectedToken,
        message: diagnostic.message,
        source: DiagnosticSource::Parse,
    }
}

fn build_syntax_tree(cst: &Cst<'_>) -> SyntaxTree {
    let mut tree = SyntaxTree {
        nodes: Vec::new(),
        debug: cst.to_string(),
    };
    push_node(cst, NodeRef::ROOT, &mut tree);
    tree
}

fn push_node(cst: &Cst<'_>, node_ref: NodeRef, tree: &mut SyntaxTree) -> usize {
    let idx = tree.nodes.len();
    let cst_kind = match cst.get(node_ref) {
        Node::Rule(rule, _) => CstKind::Rule(SyntaxRule::from(rule)),
        Node::Token(token, _) => CstKind::Token(map_token(token)),
    };
    tree.nodes.push(SyntaxNode {
        cst_kind,
        range: range(cst.span(node_ref)),
        children: Vec::new(),
    });
    let children = cst
        .children(node_ref)
        .map(|child| push_node(cst, child, tree))
        .collect();
    tree.nodes[idx].children = children;
    idx
}

macro_rules! mirrored_rule_conversions {
    ($($variant:ident),* $(,)?) => {
        impl From<Rule> for SyntaxRule {
            fn from(rule: Rule) -> Self {
                match rule {
                    $(Rule::$variant => SyntaxRule::$variant,)*
                }
            }
        }

        impl From<SyntaxRule> for Rule {
            fn from(rule: SyntaxRule) -> Self {
                match rule {
                    $(SyntaxRule::$variant => Rule::$variant,)*
                }
            }
        }
    };
}

mirrored_rule_conversions! {
    BinaryExpr,
    BinaryOperator,
    Clause,
    ClauseList,
    ComparisonOperator,
    Definition,
    Directive,
    DirectiveArgument,
    DirectiveMember,
    DirectiveName,
    DirectiveNamespace,
    Document,
    Error,
    Expr,
    FieldSelection,
    FieldSelectionTail,
    FieldSuffix,
    FragmentDef,
    FragmentSpread,
    Literal,
    LimitClause,
    OffsetClause,
    OperatorVariable,
    OrderByClause,
    OrderItem,
    QueryDef,
    QualifiedName,
    RelationRef,
    ScopedPath,
    ScopedPathSegment,
    Selection,
    SelectionSet,
    SortDirection,
    ValueVariable,
    WhereClause,
}

fn map_token(token: Token) -> SyntaxToken {
    match token {
        Token::Query => SyntaxToken::Query,
        Token::Fragment => SyntaxToken::Fragment,
        Token::On => SyntaxToken::On,
        Token::Where => SyntaxToken::Where,
        Token::Order => SyntaxToken::Order,
        Token::By => SyntaxToken::By,
        Token::Limit => SyntaxToken::Limit,
        Token::Offset => SyntaxToken::Offset,
        Token::Asc => SyntaxToken::Asc,
        Token::Desc => SyntaxToken::Desc,
        Token::True => SyntaxToken::True,
        Token::False => SyntaxToken::False,
        Token::Null => SyntaxToken::Null,
        Token::Like => SyntaxToken::Like,
        Token::And => SyntaxToken::And,
        Token::Or => SyntaxToken::Or,
        Token::LBrace => SyntaxToken::LBrace,
        Token::RBrace => SyntaxToken::RBrace,
        Token::LPar => SyntaxToken::LPar,
        Token::RPar => SyntaxToken::RPar,
        Token::LBracket => SyntaxToken::LBracket,
        Token::RBracket => SyntaxToken::RBracket,
        Token::ColonColon => SyntaxToken::ColonColon,
        Token::Arrow => SyntaxToken::Arrow,
        Token::Colon => SyntaxToken::Colon,
        Token::At => SyntaxToken::At,
        Token::Comma => SyntaxToken::Comma,
        Token::Ellipsis => SyntaxToken::Ellipsis,
        Token::DotDot => SyntaxToken::DotDot,
        Token::Dot => SyntaxToken::Dot,
        Token::Tilde => SyntaxToken::Tilde,
        Token::DollarDollar => SyntaxToken::DollarDollar,
        Token::Dollar => SyntaxToken::Dollar,
        Token::Eq => SyntaxToken::Eq,
        Token::Ne => SyntaxToken::Ne,
        Token::Gt => SyntaxToken::Gt,
        Token::Ge => SyntaxToken::Ge,
        Token::Lt => SyntaxToken::Lt,
        Token::Le => SyntaxToken::Le,
        Token::Name => SyntaxToken::Name,
        Token::String => SyntaxToken::String,
        Token::Number => SyntaxToken::Number,
        Token::Whitespace => SyntaxToken::Whitespace,
        Token::Comment => SyntaxToken::Comment,
        Token::Error => SyntaxToken::Error,
        Token::EOF => SyntaxToken::Eof,
    }
}

pub(crate) struct AstBuilder<'a> {
    cst: &'a Cst<'a>,
}

impl<'a> AstBuilder<'a> {
    fn new(cst: &'a Cst<'a>) -> Self {
        Self { cst }
    }

    fn document(&self) -> Document {
        match self.build_node(NodeRef::ROOT) {
            Some(AstNode::Document(document)) => document,
            _ => Document {
                definitions: Vec::new(),
            },
        }
    }

    pub(crate) fn selection_set(&self, node: NodeRef) -> Vec<Selection> {
        self.direct_rules(node, Rule::Selection)
            .into_iter()
            .filter_map(|selection| {
                if let Some(field) = self.direct_rule(selection, Rule::FieldSelection) {
                    return match self.build_node(field) {
                        Some(AstNode::FieldSelection(field)) => Some(Selection::Field(field)),
                        _ => None,
                    };
                }
                if let Some(spread) = self.direct_rule(selection, Rule::FragmentSpread) {
                    return match self.build_node(spread) {
                        Some(AstNode::FragmentSpread(spread)) => {
                            Some(Selection::FragmentSpread(spread))
                        }
                        _ => None,
                    };
                }
                None
            })
            .collect()
    }

    pub(crate) fn clauses(&self, node: NodeRef) -> Vec<Clause> {
        self.direct_rules(node, Rule::Clause)
            .into_iter()
            .filter_map(|clause| {
                if let Some(where_clause) = self.direct_rule(clause, Rule::WhereClause) {
                    return Some(Clause::Where(self.where_clause(where_clause)));
                }
                if let Some(order_by_clause) = self.direct_rule(clause, Rule::OrderByClause) {
                    return Some(Clause::OrderBy(self.order_by_clause(order_by_clause)));
                }
                if let Some(limit_clause) = self.direct_rule(clause, Rule::LimitClause) {
                    return Some(Clause::Limit(self.limit_clause(limit_clause)));
                }
                if let Some(offset_clause) = self.direct_rule(clause, Rule::OffsetClause) {
                    return Some(Clause::Offset(self.offset_clause(offset_clause)));
                }
                None
            })
            .collect()
    }

    pub(crate) fn directives(&self, node: NodeRef) -> Vec<Directive> {
        self.direct_rules(node, Rule::Directive)
            .into_iter()
            .filter_map(|directive| match self.build_node(directive) {
                Some(AstNode::Directive(directive)) => Some(directive),
                _ => None,
            })
            .collect()
    }

    fn where_clause(&self, node: NodeRef) -> WhereClause {
        WhereClause {
            range: range(self.cst.span(node)),
            predicate: self.direct_value_rule(node).map_or_else(
                || {
                    Expr::Literal(Literal::Null {
                        range: range(self.cst.span(node)),
                    })
                },
                |expr| self.expr(expr),
            ),
        }
    }

    fn order_by_clause(&self, node: NodeRef) -> OrderByClause {
        OrderByClause {
            range: range(self.cst.span(node)),
            items: self
                .direct_rules(node, Rule::OrderItem)
                .into_iter()
                .map(|item| self.order_by_item(item))
                .collect(),
        }
    }

    fn order_by_item(&self, node: NodeRef) -> OrderByItem {
        let direction = self
            .direct_rule(node, Rule::SortDirection)
            .map(|direction| self.sort_direction(direction))
            .unwrap_or(SortDirectionExpr::Static(SortDirection::Asc));
        OrderByItem {
            range: range(self.cst.span(node)),
            field: self
                .direct_qualified_names(node)
                .into_iter()
                .next()
                .unwrap_or_else(|| self.missing_qualified_name(node)),
            direction,
        }
    }

    fn sort_direction(&self, node: NodeRef) -> SortDirectionExpr {
        if let Some(variable) = self.direct_rule(node, Rule::ValueVariable) {
            return SortDirectionExpr::Variable(self.value_variable(variable));
        }
        let direction = self.cst.children(node).find_map(|child| {
            let (token, _, _) = token_text(self.cst, child)?;
            match token {
                Token::Asc => Some(SortDirection::Asc),
                Token::Desc => Some(SortDirection::Desc),
                _ => None,
            }
        });
        SortDirectionExpr::Static(direction.unwrap_or(SortDirection::Asc))
    }

    fn limit_clause(&self, node: NodeRef) -> LimitClause {
        LimitClause {
            range: range(self.cst.span(node)),
            value: self.direct_value_rule(node).map_or_else(
                || {
                    Expr::Literal(Literal::Null {
                        range: range(self.cst.span(node)),
                    })
                },
                |expr| self.expr(expr),
            ),
        }
    }

    fn offset_clause(&self, node: NodeRef) -> OffsetClause {
        OffsetClause {
            range: range(self.cst.span(node)),
            value: self.direct_value_rule(node).map_or_else(
                || {
                    Expr::Literal(Literal::Null {
                        range: range(self.cst.span(node)),
                    })
                },
                |expr| self.expr(expr),
            ),
        }
    }

    pub(crate) fn expr(&self, node: NodeRef) -> Expr {
        if rule(self.cst, node) == Some(Rule::BinaryExpr) {
            return self.binary_expr(node);
        }
        if rule(self.cst, node) == Some(Rule::Literal) {
            return self.literal(node);
        }
        if let Some(binary) = self.direct_rule(node, Rule::BinaryExpr) {
            return self.binary_expr(binary);
        }
        if let Some(literal) = self.direct_rule(node, Rule::Literal) {
            return self.literal(literal);
        }
        if let Some(path) = self.direct_rule(node, Rule::ScopedPath) {
            return Expr::Path(self.scoped_path(path));
        }
        if let Some(variable) = self.direct_rule(node, Rule::ValueVariable) {
            return Expr::Variable(self.value_variable(variable));
        }
        if let Some(name) = self.direct_qualified_names(node).into_iter().next() {
            return Expr::Name(name.name);
        }
        if let Some(name) = self.direct_names(node).into_iter().next() {
            return Expr::Name(name);
        }
        self.descendant_rules(node, &[Rule::BinaryExpr])
            .into_iter()
            .next()
            .map_or_else(
                || {
                    Expr::Literal(Literal::Null {
                        range: range(self.cst.span(node)),
                    })
                },
                |binary| self.binary_expr(binary),
            )
    }

    fn binary_expr(&self, node: NodeRef) -> Expr {
        let operands = self.direct_expr_operands(node);
        let left = operands.first().map_or_else(
            || {
                Expr::Literal(Literal::Null {
                    range: range(self.cst.span(node)),
                })
            },
            |expr| self.expr(*expr),
        );
        let right = operands.get(1).map_or_else(
            || {
                Expr::Literal(Literal::Null {
                    range: range(self.cst.span(node)),
                })
            },
            |expr| self.expr(*expr),
        );
        Expr::Binary {
            range: range(self.cst.span(node)),
            left: Box::new(left),
            op: self
                .binary_operator(node)
                .unwrap_or(BinaryOperator::Static(BinaryOp::Eq)),
            right: Box::new(right),
        }
    }

    fn direct_expr_operands(&self, node: NodeRef) -> Vec<NodeRef> {
        self.cst
            .children(node)
            .filter(|child| matches!(rule(self.cst, *child), Some(Rule::Expr | Rule::BinaryExpr)))
            .collect()
    }

    fn literal(&self, node: NodeRef) -> Expr {
        for child in self.cst.children(node) {
            if let Some((token, text, token_range)) = token_text(self.cst, child) {
                return Expr::Literal(match token {
                    Token::String => Literal::String {
                        range: token_range,
                        value: text.trim_matches('"').to_string(),
                    },
                    Token::Number => Literal::Number {
                        range: token_range,
                        value: text.to_string(),
                    },
                    Token::True => Literal::Bool {
                        range: token_range,
                        value: true,
                    },
                    Token::False => Literal::Bool {
                        range: token_range,
                        value: false,
                    },
                    Token::Null => Literal::Null { range: token_range },
                    _ => continue,
                });
            }
        }
        Expr::Literal(Literal::Null {
            range: range(self.cst.span(node)),
        })
    }

    fn binary_operator(&self, node: NodeRef) -> Option<BinaryOperator> {
        if let Some(operator) = self.direct_rule(node, Rule::BinaryOperator) {
            return self.binary_operator_rule(operator);
        }
        self.static_binary_op(node).map(BinaryOperator::Static)
    }

    fn binary_operator_rule(&self, node: NodeRef) -> Option<BinaryOperator> {
        if let Some(variable) = self.direct_rule(node, Rule::OperatorVariable) {
            return Some(BinaryOperator::Variable(self.operator_variable(variable)));
        }
        if let Some(comparison) = self.direct_rule(node, Rule::ComparisonOperator) {
            return self
                .static_binary_op(comparison)
                .map(BinaryOperator::Static);
        }
        self.static_binary_op(node).map(BinaryOperator::Static)
    }

    fn static_binary_op(&self, node: NodeRef) -> Option<BinaryOp> {
        self.cst
            .children(node)
            .find_map(|child| self.static_binary_op_token(child))
    }

    fn static_binary_op_token(&self, node: NodeRef) -> Option<BinaryOp> {
        let (token, _, _) = token_text(self.cst, node)?;
        match token {
            Token::Eq => Some(BinaryOp::Eq),
            Token::Ne => Some(BinaryOp::Ne),
            Token::Gt => Some(BinaryOp::Gt),
            Token::Ge => Some(BinaryOp::Ge),
            Token::Lt => Some(BinaryOp::Lt),
            Token::Le => Some(BinaryOp::Le),
            Token::Like => Some(BinaryOp::Like),
            Token::And => Some(BinaryOp::And),
            Token::Or => Some(BinaryOp::Or),
            _ => None,
        }
    }

    fn scoped_path(&self, node: NodeRef) -> ScopedPath {
        let mut scope = PathScope::Current;
        for child in self.cst.children(node) {
            let Some((token, _, _)) = token_text(self.cst, child) else {
                continue;
            };
            scope = match token {
                Token::Dot => PathScope::Current,
                Token::DotDot => PathScope::Parent,
                Token::Tilde => PathScope::Root,
                _ => continue,
            };
            break;
        }
        ScopedPath {
            range: range(self.cst.span(node)),
            scope,
            segments: self.direct_scoped_path_segments(node),
        }
    }

    fn direct_scoped_path_segments(&self, node: NodeRef) -> Vec<ScopedPathSegment> {
        self.direct_rules(node, Rule::ScopedPathSegment)
            .into_iter()
            .filter_map(|segment| {
                let name = self.direct_qualified_names(segment).into_iter().next()?;
                let selector = self.edge_selector(segment);
                let end = selector
                    .as_ref()
                    .map_or(name.range.end, |selector| selector.range.end);
                Some(ScopedPathSegment {
                    range: TextRange {
                        start: name.range.start,
                        end,
                    },
                    schema: name.schema,
                    name: name.name,
                    selector,
                })
            })
            .collect()
    }

    pub(crate) fn direct_names(&self, node: NodeRef) -> Vec<NameRef> {
        self.cst
            .children(node)
            .filter_map(|child| {
                let (token, text, token_range) = token_text(self.cst, child)?;
                (token == Token::Name).then(|| NameRef {
                    range: token_range,
                    text: text.to_string(),
                })
            })
            .collect()
    }

    /// Returns the direct child token when the first child of `node` is a token.
    pub(crate) fn first_direct_token(&self, node: NodeRef) -> Option<(Token, TextRange)> {
        self.cst
            .children(node)
            .next()
            .and_then(|child| token_text(self.cst, child).map(|(token, _, range)| (token, range)))
    }

    pub(crate) fn direct_qualified_names(&self, node: NodeRef) -> Vec<QualifiedNameRef> {
        self.direct_rules(node, Rule::QualifiedName)
            .into_iter()
            .filter_map(|qualified| {
                let names = self.direct_names(qualified);
                let name = names.last()?.clone();
                let schema = (names.len() > 1).then(|| names[0].clone());
                Some(QualifiedNameRef {
                    range: range(self.cst.span(qualified)),
                    schema,
                    name,
                })
            })
            .collect()
    }

    pub(crate) fn direct_relation_refs(&self, node: NodeRef) -> Vec<RelationRef> {
        self.direct_rules(node, Rule::RelationRef)
            .into_iter()
            .filter_map(|relation| self.relation_ref(relation))
            .collect()
    }

    fn relation_ref(&self, relation: NodeRef) -> Option<RelationRef> {
        let qualified = self.direct_qualified_names(relation).into_iter().next()?;
        let selector = self.edge_selector(relation);
        Some(RelationRef {
            range: TextRange {
                start: qualified.range.start,
                end: selector
                    .as_ref()
                    .map_or(qualified.range.end, |selector| selector.range.end),
            },
            target: qualified,
            selector,
        })
    }

    fn edge_selector(&self, node: NodeRef) -> Option<NameRef> {
        self.cst
            .children(node)
            .filter_map(|child| token_text(self.cst, child))
            .scan(false, |after_arrow, (token, text, token_range)| {
                if *after_arrow && token == Token::Name {
                    return Some(Some(NameRef {
                        range: token_range,
                        text: text.to_string(),
                    }));
                }
                *after_arrow = token == Token::Arrow;
                Some(None)
            })
            .flatten()
            .next()
    }

    pub(crate) fn direct_rule(&self, node: NodeRef, target: Rule) -> Option<NodeRef> {
        self.direct_rules(node, target).into_iter().next()
    }

    pub(crate) fn direct_value_rule(&self, node: NodeRef) -> Option<NodeRef> {
        self.cst.children(node).find(|child| {
            matches!(
                rule(self.cst, *child),
                Some(
                    Rule::Expr
                        | Rule::BinaryExpr
                        | Rule::Literal
                        | Rule::QualifiedName
                        | Rule::ScopedPath
                        | Rule::ValueVariable
                )
            )
        })
    }

    fn value_variable(&self, node: NodeRef) -> ValueVariable {
        let mut scope = VariableScope::Structured;
        for child in self.cst.children(node) {
            let Some((token, _, _)) = token_text(self.cst, child) else {
                continue;
            };
            scope = match token {
                Token::Dollar => VariableScope::Structured,
                Token::DollarDollar => VariableScope::TopLevel,
                _ => continue,
            };
            break;
        }
        ValueVariable {
            range: range(self.cst.span(node)),
            scope,
            name: self.direct_names(node).into_iter().next(),
        }
    }

    fn operator_variable(&self, node: NodeRef) -> OperatorVariable {
        let mut scope = VariableScope::Structured;
        for child in self.cst.children(node) {
            let Some((token, _, _)) = token_text(self.cst, child) else {
                continue;
            };
            scope = match token {
                Token::Dollar => VariableScope::Structured,
                Token::DollarDollar => VariableScope::TopLevel,
                _ => continue,
            };
            break;
        }
        OperatorVariable {
            range: range(self.cst.span(node)),
            scope,
            name: self.direct_names(node).into_iter().next(),
            allowed: self
                .direct_rules(node, Rule::ComparisonOperator)
                .into_iter()
                .filter_map(|operator| self.static_binary_op(operator))
                .collect(),
        }
    }

    pub(crate) fn direct_rules(&self, node: NodeRef, target: Rule) -> Vec<NodeRef> {
        self.cst
            .children(node)
            .filter(|child| rule(self.cst, *child) == Some(target))
            .collect()
    }

    pub(crate) fn descendant_rules(&self, node: NodeRef, targets: &[Rule]) -> Vec<NodeRef> {
        let mut out = Vec::new();
        self.collect_descendant_rules(node, targets, &mut out);
        out
    }

    fn collect_descendant_rules(&self, node: NodeRef, targets: &[Rule], out: &mut Vec<NodeRef>) {
        for child in self.cst.children(node) {
            if let Some(child_rule) = rule(self.cst, child)
                && targets.contains(&child_rule)
            {
                out.push(child);
                continue;
            }
            self.collect_descendant_rules(child, targets, out);
        }
    }

    pub(crate) fn node_range(&self, node: NodeRef) -> TextRange {
        range(self.cst.span(node))
    }

    /// Builds the atom-owned AST node for a CST node by grammar-rule lookup.
    pub(crate) fn build_node(&self, node: NodeRef) -> Option<AstNode> {
        let rule = rule(self.cst, node)?;
        LanguageAtoms::ast_builder_for_rule(rule).map(|builder| builder.build(self, node))
    }

    /// Builds the first direct child owned by `target` through the atom registry.
    #[expect(
        dead_code,
        reason = "available for atoms as more syntax constructs migrate into registry dispatch"
    )]
    pub(crate) fn build_child(&self, node: NodeRef, target: Rule) -> Option<AstNode> {
        self.direct_rule(node, target)
            .and_then(|child| self.build_node(child))
    }

    /// Builds direct children owned by `target` through the atom registry.
    #[expect(
        dead_code,
        reason = "available for atoms as more syntax constructs migrate into registry dispatch"
    )]
    pub(crate) fn build_children(&self, node: NodeRef, target: Rule) -> Vec<AstNode> {
        self.direct_rules(node, target)
            .into_iter()
            .filter_map(|child| self.build_node(child))
            .collect()
    }

    pub(crate) fn missing_name(&self, node: NodeRef) -> NameRef {
        NameRef {
            range: range(self.cst.span(node)),
            text: "<missing>".to_string(),
        }
    }

    fn missing_qualified_name(&self, node: NodeRef) -> QualifiedNameRef {
        let name = self.missing_name(node);
        QualifiedNameRef {
            range: name.range,
            schema: None,
            name,
        }
    }

    pub(crate) fn missing_relation_ref(&self, node: NodeRef) -> RelationRef {
        let target = self.missing_qualified_name(node);
        RelationRef {
            range: target.range,
            target,
            selector: None,
        }
    }
}

fn rule(cst: &Cst<'_>, node: NodeRef) -> Option<Rule> {
    match cst.get(node) {
        Node::Rule(rule, _) => Some(rule),
        Node::Token(_, _) => None,
    }
}

fn token_text<'a>(cst: &'a Cst<'a>, node: NodeRef) -> Option<(Token, &'a str, TextRange)> {
    match cst.get(node) {
        Node::Token(token, _) => {
            let span = cst.span(node);
            Some((token, &cst.source()[span.clone()], range(span)))
        }
        Node::Rule(_, _) => None,
    }
}

fn range(span: std::ops::Range<usize>) -> TextRange {
    TextRange::new(span.start, span.end)
}

impl std::fmt::Display for SyntaxTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.debug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::RopeBuilder;

    fn first_field(query: &crate::syntax::QueryDef) -> &crate::FieldSelection {
        query.selections[0]
            .as_field()
            .expect("expected first selection to be a field")
    }

    #[test]
    fn parses_query_from_text() {
        let src = "query Users { users(where .id > 18 order by name desc limit 10 offset 2) { id, name } }";
        let parsed = parse_source(SourceSnapshot::from(src));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(parsed.source_file.queries().count(), 1);
        let query = parsed.source_file.queries().next().unwrap();
        assert_eq!(first_field(query).clauses.len(), 4);
    }

    #[test]
    fn parses_scoped_predicate_paths_into_segments() {
        let src = "query Users { users(where .posts.title like \"%foo%\") { id } }";
        let parsed = parse_source(SourceSnapshot::from(src));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let query = parsed.source_file.queries().next().unwrap();
        let Clause::Where(where_clause) = &first_field(query).clauses[0] else {
            panic!("expected where clause");
        };
        let Expr::Binary { left, op, .. } = &where_clause.predicate else {
            panic!("expected binary predicate");
        };
        assert_eq!(*op, BinaryOperator::Static(BinaryOp::Like));
        let Expr::Path(path) = left.as_ref() else {
            panic!("expected scoped path");
        };
        assert_eq!(path.scope, PathScope::Current);
        assert_eq!(
            path.segments
                .iter()
                .map(|segment| segment.display_text())
                .collect::<Vec<_>>(),
            ["posts", "title"],
        );
    }

    #[test]
    fn parses_scoped_relationship_selector_paths_into_segments() {
        let src = "query Titles { title(where .aka_title->movie_id.title like \"%foo%\") { id } }";
        let parsed = parse_source(SourceSnapshot::from(src));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let query = parsed.source_file.queries().next().unwrap();
        let Clause::Where(where_clause) = &first_field(query).clauses[0] else {
            panic!("expected where clause");
        };
        let Expr::Binary { left, op, .. } = &where_clause.predicate else {
            panic!("expected binary predicate");
        };
        assert_eq!(*op, BinaryOperator::Static(BinaryOp::Like));
        let Expr::Path(path) = left.as_ref() else {
            panic!("expected scoped path");
        };
        assert_eq!(path.scope, PathScope::Current);
        assert_eq!(
            path.segments
                .iter()
                .map(|segment| segment.display_text())
                .collect::<Vec<_>>(),
            ["aka_title->movie_id", "title"],
        );
        assert_eq!(path.segments[0].name.text, "aka_title");
        assert_eq!(
            path.segments[0]
                .selector
                .as_ref()
                .map(|selector| selector.text.as_str()),
            Some("movie_id"),
        );
    }

    #[test]
    fn rejects_anonymous_queries() {
        let parsed = parse_source(SourceSnapshot::from("query { users { id } }"));
        assert!(!parsed.diagnostics.is_empty());
    }

    #[test]
    fn parses_query_from_multi_chunk_rope_snapshot() {
        let mut builder = RopeBuilder::new();
        for _ in 0..2048 {
            builder.append("# source padding\n");
        }
        builder.append("query Users { users { id } }");
        let rope = builder.finish();
        assert!(rope.chunks().nth(1).is_some());

        let parsed = parse_source(SourceSnapshot::from_rope(rope));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(parsed.source_file.queries().count(), 1);
    }

    #[test]
    fn reports_malformed_input() {
        let parsed = parse_source(SourceSnapshot::from("query Users { users("));
        assert!(!parsed.diagnostics.is_empty());
    }

    #[test]
    fn reports_expected_clause_tokens_at_cursor() {
        let src = "query Users { users() { id } }";
        let byte = src.find("()").unwrap() + 1;
        let expected = expected_tokens_at(&SourceSnapshot::from(src), byte);

        assert!(expected.contains(&SyntaxToken::Where), "{expected:?}");
        assert!(expected.contains(&SyntaxToken::Order), "{expected:?}");
        assert!(expected.contains(&SyntaxToken::Limit), "{expected:?}");
        assert!(expected.contains(&SyntaxToken::Offset), "{expected:?}");
    }

    #[test]
    fn reports_expected_name_after_where_at_cursor() {
        let src = "query Users { users(where ) { id } }";
        let byte = src.find("where ").unwrap() + "where ".len();
        let expected = expected_tokens_at(&SourceSnapshot::from(src), byte);

        assert!(expected.contains(&SyntaxToken::Dot), "{expected:?}");
        assert!(expected.contains(&SyntaxToken::Tilde), "{expected:?}");
    }
}
