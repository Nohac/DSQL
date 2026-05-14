use super::ast::*;
use super::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSource, Severity};
use super::grammar::lexer::Token;
use super::grammar::parser::{Cst, Node, NodeRef, Parser, Rule};
use super::{SourceFile, SourceSnapshot, TextRange};
use facet::Facet;

#[derive(Clone, Debug)]
pub struct ParseResult {
    pub source: SourceSnapshot,
    pub tree: SyntaxTree,
    pub source_file: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Facet)]
pub struct SyntaxTree {
    pub nodes: Vec<SyntaxNode>,
    pub debug: String,
}

#[derive(Clone, Debug, Facet)]
pub struct SyntaxNode {
    pub kind: AstNode,
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
    Argument,
    ArgumentList,
    BinaryExpr,
    Definition,
    Directive,
    Document,
    Error,
    Expr,
    FieldSelection,
    FieldSelectionTail,
    FieldSuffix,
    FragmentSpread,
    FragmentDef,
    Literal,
    QueryDef,
    QualifiedName,
    Selection,
    SelectionSet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum SyntaxToken {
    Query,
    Fragment,
    On,
    True,
    False,
    Null,
    LBrace,
    RBrace,
    LPar,
    RPar,
    Colon,
    At,
    Comma,
    Ellipsis,
    Dot,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum AstNode {
    Document,
    Query,
    Fragment,
    Selection,
    Argument,
    Expression,
    Error,
}

pub fn parse_source(source: SourceSnapshot) -> ParseResult {
    // Lelwel's generated runtime is &str-based. Keep Arc<str> as the parser
    // source for now; Rope-backed snapshots remain a frontend/LSP boundary until
    // we patch or replace the generated source storage.
    let source_text = source.to_arc_str();
    let mut lelwel_diagnostics = Vec::new();
    let cst = Parser::new(&source_text, &mut lelwel_diagnostics).parse(&mut lelwel_diagnostics);
    let diagnostics = lelwel_diagnostics
        .into_iter()
        .map(convert_diagnostic)
        .collect::<Vec<_>>();
    let document = AstBuilder::new(&cst).document();
    let tree = build_syntax_tree(&cst);

    ParseResult {
        source,
        tree,
        source_file: SourceFile::new(document),
        diagnostics,
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
        Node::Rule(rule, _) => CstKind::Rule(map_rule(rule)),
        Node::Token(token, _) => CstKind::Token(map_token(token)),
    };
    let kind = match cst_kind {
        CstKind::Rule(SyntaxRule::Document) => AstNode::Document,
        CstKind::Rule(SyntaxRule::QueryDef) => AstNode::Query,
        CstKind::Rule(SyntaxRule::FragmentDef) => AstNode::Fragment,
        CstKind::Rule(SyntaxRule::Selection | SyntaxRule::FieldSelection) => AstNode::Selection,
        CstKind::Rule(SyntaxRule::Argument) => AstNode::Argument,
        CstKind::Rule(SyntaxRule::Expr | SyntaxRule::BinaryExpr | SyntaxRule::Literal) => {
            AstNode::Expression
        }
        CstKind::Rule(SyntaxRule::Error) => AstNode::Error,
        CstKind::Rule(_) | CstKind::Token(_) => AstNode::Expression,
    };
    tree.nodes.push(SyntaxNode {
        kind,
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

fn map_rule(rule: Rule) -> SyntaxRule {
    match rule {
        Rule::Argument => SyntaxRule::Argument,
        Rule::ArgumentList => SyntaxRule::ArgumentList,
        Rule::BinaryExpr => SyntaxRule::BinaryExpr,
        Rule::Definition => SyntaxRule::Definition,
        Rule::Directive => SyntaxRule::Directive,
        Rule::Document => SyntaxRule::Document,
        Rule::Error => SyntaxRule::Error,
        Rule::Expr => SyntaxRule::Expr,
        Rule::FieldSelection => SyntaxRule::FieldSelection,
        Rule::FieldSelectionTail => SyntaxRule::FieldSelectionTail,
        Rule::FieldSuffix => SyntaxRule::FieldSuffix,
        Rule::FragmentDef => SyntaxRule::FragmentDef,
        Rule::FragmentSpread => SyntaxRule::FragmentSpread,
        Rule::Literal => SyntaxRule::Literal,
        Rule::QueryDef => SyntaxRule::QueryDef,
        Rule::QualifiedName => SyntaxRule::QualifiedName,
        Rule::Selection => SyntaxRule::Selection,
        Rule::SelectionSet => SyntaxRule::SelectionSet,
    }
}

fn map_token(token: Token) -> SyntaxToken {
    match token {
        Token::Query => SyntaxToken::Query,
        Token::Fragment => SyntaxToken::Fragment,
        Token::On => SyntaxToken::On,
        Token::True => SyntaxToken::True,
        Token::False => SyntaxToken::False,
        Token::Null => SyntaxToken::Null,
        Token::LBrace => SyntaxToken::LBrace,
        Token::RBrace => SyntaxToken::RBrace,
        Token::LPar => SyntaxToken::LPar,
        Token::RPar => SyntaxToken::RPar,
        Token::Colon => SyntaxToken::Colon,
        Token::At => SyntaxToken::At,
        Token::Comma => SyntaxToken::Comma,
        Token::Ellipsis => SyntaxToken::Ellipsis,
        Token::Dot => SyntaxToken::Dot,
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

struct AstBuilder<'a> {
    cst: &'a Cst<'a>,
}

impl<'a> AstBuilder<'a> {
    fn new(cst: &'a Cst<'a>) -> Self {
        Self { cst }
    }

    fn document(&self) -> Document {
        let definitions = self
            .descendant_rules(NodeRef::ROOT, &[Rule::QueryDef, Rule::FragmentDef])
            .into_iter()
            .filter_map(|node| match rule(self.cst, node) {
                Some(Rule::QueryDef) => Some(Definition::Query(self.query(node))),
                Some(Rule::FragmentDef) => Some(Definition::Fragment(self.fragment(node))),
                _ => None,
            })
            .collect();
        Document { definitions }
    }

    fn query(&self, node: NodeRef) -> QueryDef {
        let names = self.direct_names(node);
        QueryDef {
            range: range(self.cst.span(node)),
            name: names.first().cloned(),
            selections: self
                .direct_rule(node, Rule::SelectionSet)
                .map_or_else(Vec::new, |selection_set| self.selection_set(selection_set)),
        }
    }

    fn fragment(&self, node: NodeRef) -> FragmentDef {
        let names = self.direct_names(node);
        let qualified_names = self.direct_qualified_names(node);
        FragmentDef {
            range: range(self.cst.span(node)),
            name: names.first().cloned(),
            on: qualified_names.first().cloned(),
            selections: self
                .direct_rule(node, Rule::SelectionSet)
                .map_or_else(Vec::new, |selection_set| self.selection_set(selection_set)),
        }
    }

    fn selection_set(&self, node: NodeRef) -> Vec<Selection> {
        self.direct_rules(node, Rule::Selection)
            .into_iter()
            .filter_map(|selection| {
                self.direct_rule(selection, Rule::FieldSelection)
                    .map(|field| self.field_selection(field))
                    .or_else(|| {
                        self.direct_rule(selection, Rule::FragmentSpread)
                            .map(|spread| self.fragment_spread(spread))
                    })
            })
            .collect()
    }

    fn field_selection(&self, node: NodeRef) -> Selection {
        let first_name = self.direct_qualified_names(node).into_iter().next();
        let tail = self.direct_rule(node, Rule::FieldSelectionTail);
        let (alias, name, suffix) = if let Some(tail) = tail {
            let tail_names = self.direct_qualified_names(tail);
            if tail_names.is_empty() {
                (
                    None,
                    first_name.unwrap_or_else(|| self.missing_name(node)),
                    self.direct_rule(tail, Rule::FieldSuffix),
                )
            } else {
                (
                    first_name.map(|name| NameRef {
                        range: name.range,
                        text: name.text,
                    }),
                    tail_names
                        .first()
                        .cloned()
                        .unwrap_or_else(|| self.missing_name(tail)),
                    self.direct_rule(tail, Rule::FieldSuffix),
                )
            }
        } else {
            (
                None,
                first_name.unwrap_or_else(|| self.missing_name(node)),
                None,
            )
        };
        let arguments = suffix
            .and_then(|suffix| self.direct_rule(suffix, Rule::ArgumentList))
            .map_or_else(Vec::new, |args| self.arguments(args));
        let directives = suffix.map_or_else(Vec::new, |suffix| {
            self.direct_rules(suffix, Rule::Directive)
                .into_iter()
                .filter_map(|directive| self.direct_names(directive).into_iter().next())
                .collect()
        });
        let selections = suffix
            .and_then(|suffix| self.direct_rule(suffix, Rule::SelectionSet))
            .map_or_else(Vec::new, |selection_set| self.selection_set(selection_set));

        Selection {
            range: range(self.cst.span(node)),
            kind: SelectionKind::Field,
            alias,
            name,
            arguments,
            directives,
            selections,
        }
    }

    fn fragment_spread(&self, node: NodeRef) -> Selection {
        let name = self
            .direct_names(node)
            .into_iter()
            .next()
            .unwrap_or_else(|| self.missing_name(node));
        let directives = self
            .direct_rules(node, Rule::Directive)
            .into_iter()
            .filter_map(|directive| self.direct_names(directive).into_iter().next())
            .collect();
        Selection {
            range: range(self.cst.span(node)),
            kind: SelectionKind::FragmentSpread,
            alias: None,
            name,
            arguments: Vec::new(),
            directives,
            selections: Vec::new(),
        }
    }

    fn arguments(&self, node: NodeRef) -> Vec<Argument> {
        self.direct_rules(node, Rule::Argument)
            .into_iter()
            .map(|argument| {
                let name = self
                    .direct_names(argument)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.missing_name(argument));
                let value = self.direct_value_rule(argument).map_or_else(
                    || Expr::Literal(Literal::Null { range: name.range }),
                    |expr| self.expr(expr),
                );
                Argument {
                    range: range(self.cst.span(argument)),
                    name,
                    value,
                }
            })
            .collect()
    }

    fn expr(&self, node: NodeRef) -> Expr {
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
        let exprs = self.direct_rules(node, Rule::Expr);
        let left = exprs.first().map_or_else(
            || {
                Expr::Literal(Literal::Null {
                    range: range(self.cst.span(node)),
                })
            },
            |expr| self.expr(*expr),
        );
        let right = exprs.get(1).map_or_else(
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
            op: self.binary_op(node).unwrap_or(BinaryOp::Eq),
            right: Box::new(right),
        }
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

    fn binary_op(&self, node: NodeRef) -> Option<BinaryOp> {
        self.cst.children(node).find_map(|child| {
            let (token, _, _) = token_text(self.cst, child)?;
            match token {
                Token::Eq => Some(BinaryOp::Eq),
                Token::Ne => Some(BinaryOp::Ne),
                Token::Gt => Some(BinaryOp::Gt),
                Token::Ge => Some(BinaryOp::Ge),
                Token::Lt => Some(BinaryOp::Lt),
                Token::Le => Some(BinaryOp::Le),
                _ => None,
            }
        })
    }

    fn direct_names(&self, node: NodeRef) -> Vec<NameRef> {
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

    fn direct_qualified_names(&self, node: NodeRef) -> Vec<NameRef> {
        self.direct_rules(node, Rule::QualifiedName)
            .into_iter()
            .filter_map(|qualified| {
                let names = self.direct_names(qualified);
                if names.is_empty() {
                    return None;
                }
                let text = names
                    .iter()
                    .map(|name| name.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".");
                Some(NameRef {
                    range: range(self.cst.span(qualified)),
                    text,
                })
            })
            .collect()
    }

    fn direct_rule(&self, node: NodeRef, target: Rule) -> Option<NodeRef> {
        self.direct_rules(node, target).into_iter().next()
    }

    fn direct_value_rule(&self, node: NodeRef) -> Option<NodeRef> {
        self.cst.children(node).find(|child| {
            matches!(
                rule(self.cst, *child),
                Some(Rule::Expr | Rule::BinaryExpr | Rule::Literal)
            )
        })
    }

    fn direct_rules(&self, node: NodeRef, target: Rule) -> Vec<NodeRef> {
        self.cst
            .children(node)
            .filter(|child| rule(self.cst, *child) == Some(target))
            .collect()
    }

    fn descendant_rules(&self, node: NodeRef, targets: &[Rule]) -> Vec<NodeRef> {
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

    fn missing_name(&self, node: NodeRef) -> NameRef {
        NameRef {
            range: range(self.cst.span(node)),
            text: "<missing>".to_string(),
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
    use ropey::Rope;

    #[test]
    fn parses_query_from_text() {
        let src = "query Users { users(where age > 18) { id name } }";
        let parsed = parse_source(SourceSnapshot::from(src));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(parsed.source_file.queries().count(), 1);
    }

    #[test]
    fn rope_snapshots_are_flattened_for_lelwel_for_now() {
        let src = "query Users { users { id } }";
        let parsed = parse_source(SourceSnapshot::from_rope(Rope::from_str(src)));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(parsed.source_file.queries().count(), 1);
    }

    #[test]
    fn reports_malformed_input() {
        let parsed = parse_source(SourceSnapshot::from("query Users { users("));
        assert!(!parsed.diagnostics.is_empty());
    }
}
