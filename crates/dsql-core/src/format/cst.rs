//! The CST formatter engine.
//!
//! Conservative by design: comments survive, user line breaks inside clause
//! lists are respected, and a file with parse errors is returned unchanged
//! (`FormatConfidence::PreserveOriginal`) rather than half-formatted.
//! Per-construct formatting lives with the owning entity (`FormatStage`);
//! this module owns the engine — indentation, line-width decisions, and the
//! expression/clause layout machinery entities share.

use crate::entities::format_rule;
use crate::facts::Span;
use crate::grammar::lexer::Token;
use crate::grammar::parser::{CstData, Node, NodeRef, Rule};

const DEFAULT_FORMAT_LINE_WIDTH: usize = 100;

/// How much of the input the formatter was confident enough to rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatConfidence {
    /// The whole file was formatted.
    Full,
    /// Parse errors: the original text is returned untouched.
    PreserveOriginal,
}

/// The outcome of formatting one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedText {
    pub text: String,
    pub confidence: FormatConfidence,
}

/// Formats a parsed file. `has_parse_errors` comes from the parse that
/// produced `cst`; when set, the original text is preserved.
pub fn format_document(cst: &CstData, source: &str, has_parse_errors: bool) -> FormattedText {
    if has_parse_errors {
        return FormattedText {
            text: source.to_string(),
            confidence: FormatConfidence::PreserveOriginal,
        };
    }

    let mut formatter = CstFormatter::new(cst, source);
    formatter.format_child(NodeRef::ROOT);
    FormattedText {
        text: formatter.finish(),
        confidence: FormatConfidence::Full,
    }
}

/// The shared formatting state handed to entity `FormatStage` impls.
pub struct CstFormatter<'a> {
    cst: &'a CstData,
    source: &'a str,
    out: String,
    indent: usize,
}

impl<'a> CstFormatter<'a> {
    fn new(cst: &'a CstData, source: &'a str) -> Self {
        Self {
            cst,
            source,
            out: String::new(),
            indent: 0,
        }
    }

    pub fn selection_set(&mut self, node: NodeRef) {
        self.out.push_str(" {\n");
        self.indent += 1;
        let children: Vec<NodeRef> = self.children(node);
        let mut idx = 0;
        while idx < children.len() {
            let child = children[idx];
            match (self.rule(child), self.token(child)) {
                (Some(Rule::Selection), _) => {
                    self.write_indent(self.indent);
                    self.selection(child);
                    let mut current = child;
                    while self.selection_has_comma(current) {
                        let Some(next_idx) = children
                            .iter()
                            .enumerate()
                            .skip(idx + 1)
                            .find_map(|(next_idx, next)| {
                                (self.rule(*next) == Some(Rule::Selection)).then_some(next_idx)
                            })
                        else {
                            break;
                        };
                        let next = children[next_idx];
                        if self.text_between(current, next).contains('\n') {
                            break;
                        }
                        self.out.push_str(", ");
                        self.selection(next);
                        current = next;
                        idx = next_idx;
                    }
                    self.out.push('\n');
                }
                (_, Some(Token::Comment)) => {
                    self.write_indent(self.indent);
                    self.write_node_text(child);
                    self.out.push('\n');
                }
                _ => {}
            }
            idx += 1;
        }
        self.indent = self.indent.saturating_sub(1);
        self.write_indent(self.indent);
        self.out.push('}');
    }

    fn selection(&mut self, node: NodeRef) {
        if let Some(field) = self.direct_rule(node, Rule::FieldSelection) {
            self.format_child(field);
        } else if let Some(spread) = self.direct_rule(node, Rule::FragmentSpread) {
            self.format_child(spread);
        }
    }

    pub fn field_suffix(&mut self, node: NodeRef) {
        if let Some(clauses) = self.direct_rule(node, Rule::ClauseList) {
            self.clause_list(clauses);
        }
        for directive in self.direct_rules(node, Rule::Directive) {
            self.format_child(directive);
        }
        if let Some(selection_set) = self.direct_rule(node, Rule::SelectionSet) {
            self.selection_set(selection_set);
        }
    }

    fn clause_list(&mut self, node: NodeRef) {
        let clauses = self.direct_rules(node, Rule::Clause);
        if let Some((where_clause, rest)) = self.complex_where_clause(&clauses) {
            self.out.push('(');
            self.where_clause_multiline(where_clause, self.indent + 1);
            if !rest.is_empty() {
                self.out.push('\n');
                self.write_indent(self.indent + 1);
                for (idx, clause) in rest.into_iter().enumerate() {
                    if idx > 0 {
                        self.out.push(' ');
                    }
                    self.clause(clause);
                }
            }
            self.out.push('\n');
            self.write_indent(self.indent);
            self.out.push(')');
            return;
        }

        if self.clause_list_has_linebreaks(&clauses) {
            self.clause_list_multiline(&clauses, true);
            return;
        }

        let inline = self.inline_clause_list_text(&clauses);
        if self.current_line_len() + inline.len() <= DEFAULT_FORMAT_LINE_WIDTH {
            self.out.push_str(&inline);
        } else {
            self.clause_list_multiline(&clauses, false);
        }
    }

    fn inline_clause_list_text(&self, clauses: &[NodeRef]) -> String {
        let mut formatter = self.empty_like();
        formatter.out.push('(');
        for (idx, clause) in clauses.iter().copied().enumerate() {
            if idx > 0 {
                formatter.out.push(' ');
            }
            formatter.clause(clause);
        }
        formatter.out.push(')');
        formatter.out
    }

    fn clause_text(&self, clause: NodeRef) -> String {
        let mut formatter = self.empty_like();
        formatter.clause(clause);
        formatter.out
    }

    fn clause_list_multiline(&mut self, clauses: &[NodeRef], preserve_user_breaks: bool) {
        self.out.push('(');
        for (idx, clause) in clauses.iter().copied().enumerate() {
            if idx > 0 {
                let previous = clauses[idx - 1];
                let clause_text = self.clause_text(clause);
                let user_split =
                    preserve_user_breaks && self.text_between(previous, clause).contains('\n');
                let split_after_where =
                    !preserve_user_breaks && idx == 1 && self.is_where_clause(clauses[0]);
                if user_split
                    || split_after_where
                    || self.current_line_len() + 1 + clause_text.len() > DEFAULT_FORMAT_LINE_WIDTH
                {
                    self.out.push('\n');
                    self.write_indent(self.indent + 1);
                } else {
                    self.out.push(' ');
                }
            }
            self.clause(clause);
        }
        self.out.push('\n');
        self.write_indent(self.indent);
        self.out.push(')');
    }

    fn complex_where_clause(&self, clauses: &[NodeRef]) -> Option<(NodeRef, Vec<NodeRef>)> {
        let first = *clauses.first()?;
        let where_clause = self.direct_rule(first, Rule::WhereClause)?;
        let value = self.direct_value_rule(where_clause)?;
        self.is_complex_predicate(value)
            .then(|| (where_clause, clauses[1..].to_vec()))
    }

    fn clause_list_has_linebreaks(&self, clauses: &[NodeRef]) -> bool {
        clauses.windows(2).any(|pair| {
            let [left, right] = pair else {
                return false;
            };
            self.text_between(*left, *right).contains('\n')
        })
    }

    fn where_clause_multiline(&mut self, node: NodeRef, continuation_indent: usize) {
        self.out.push_str("where ");
        if let Some(value) = self.direct_value_rule(node) {
            self.expr_multiline(value, continuation_indent);
        }
    }

    fn is_where_clause(&self, node: NodeRef) -> bool {
        self.direct_rule(node, Rule::WhereClause).is_some()
    }

    pub fn clause(&mut self, node: NodeRef) {
        if let Some(where_clause) = self.direct_rule(node, Rule::WhereClause) {
            self.format_child(where_clause);
        } else if let Some(order_by) = self.direct_rule(node, Rule::OrderByClause) {
            self.format_child(order_by);
        } else if let Some(limit) = self.direct_rule(node, Rule::LimitClause) {
            self.format_child(limit);
        } else if let Some(offset) = self.direct_rule(node, Rule::OffsetClause) {
            self.format_child(offset);
        }
    }

    pub fn order_item(&mut self, node: NodeRef) {
        if let Some(name) = self.direct_qualified_name_text(node) {
            self.out.push_str(&name);
        }
        if let Some(direction) = self.direct_rule(node, Rule::SortDirection) {
            self.out.push(' ');
            self.write_node_text(direction);
        }
    }

    pub fn expr(&mut self, node: NodeRef) {
        match self.rule(node) {
            Some(Rule::BinaryExpr) => self.binary_expr(node),
            Some(Rule::Literal) => self.literal(node),
            Some(Rule::Expr) => {
                let grouped = self.is_grouped_expr(node);
                if grouped {
                    self.out.push('(');
                }
                if let Some(binary) = self.direct_rule(node, Rule::BinaryExpr) {
                    self.binary_expr(binary);
                } else if let Some(literal) = self.direct_rule(node, Rule::Literal) {
                    self.literal(literal);
                } else if let Some(path) = self.direct_rule(node, Rule::ScopedPath) {
                    self.write_node_text(path);
                } else if let Some(variable) = self.direct_rule(node, Rule::ValueVariable) {
                    self.write_node_text(variable);
                } else if let Some(inner) = self.direct_rule(node, Rule::Expr) {
                    self.expr(inner);
                } else if let Some(name) = self.direct_qualified_name_text(node) {
                    self.out.push_str(&name);
                }
                if grouped {
                    self.out.push(')');
                }
            }
            Some(Rule::QualifiedName) => {
                if let Some(name) = self.qualified_name_text(node) {
                    self.out.push_str(&name);
                }
            }
            Some(Rule::ScopedPath) | Some(Rule::ValueVariable) => self.write_node_text(node),
            _ => {}
        }
    }

    fn expr_multiline(&mut self, node: NodeRef, line_indent: usize) {
        if self.is_grouped_expr(node) {
            self.out.push('(');
            self.out.push('\n');
            self.write_indent(line_indent + 1);
            if let Some(inner) = self.direct_value_rule(node) {
                self.expr_multiline(inner, line_indent + 1);
            }
            self.out.push('\n');
            self.write_indent(line_indent);
            self.out.push(')');
            return;
        }

        if self.rule(node) == Some(Rule::BinaryExpr)
            && let Some(op) = self.direct_operator(node)
            && matches!(self.token(op), Some(Token::And | Token::Or))
        {
            let exprs = self.direct_expr_operands(node);
            if let Some(left) = exprs.first().copied() {
                self.expr_multiline(left, line_indent);
            }
            if let Some(right) = exprs.get(1).copied() {
                self.out.push('\n');
                self.write_indent(line_indent);
                self.write_node_text(op);
                self.out.push(' ');
                if self.is_grouped_expr(right) {
                    self.expr_multiline(right, line_indent);
                } else {
                    self.expr(right);
                }
            }
            return;
        }

        self.expr(node);
    }

    fn binary_expr(&mut self, node: NodeRef) {
        let exprs = self.direct_expr_operands(node);
        if let Some(left) = exprs.first().copied() {
            self.expr(left);
        }
        if let Some(op) = self.direct_operator(node) {
            self.out.push(' ');
            self.write_node_text(op);
            self.out.push(' ');
        }
        if let Some(right) = exprs.get(1).copied() {
            self.expr(right);
        }
    }

    fn direct_expr_operands(&self, node: NodeRef) -> Vec<NodeRef> {
        self.children(node)
            .into_iter()
            .filter(|child| matches!(self.rule(*child), Some(Rule::Expr | Rule::BinaryExpr)))
            .collect()
    }

    fn is_complex_predicate(&self, node: NodeRef) -> bool {
        self.is_grouped_expr(node)
            || self.children(node).into_iter().any(|child| {
                matches!(self.token(child), Some(Token::And | Token::Or))
                    || self.is_complex_predicate(child)
            })
    }

    fn is_grouped_expr(&self, node: NodeRef) -> bool {
        self.rule(node) == Some(Rule::Expr)
            && self
                .children(node)
                .into_iter()
                .any(|child| self.token(child) == Some(Token::LPar))
    }

    fn literal(&mut self, node: NodeRef) {
        if let Some(token) = self.children(node).into_iter().find(|child| {
            matches!(self.node(*child), Node::Token(token, _)
                if !matches!(token, Token::Whitespace | Token::Comment))
        }) {
            self.write_node_text(token);
        }
    }

    /// Writes the blank line separating top-level definitions.
    pub fn blank_between_definitions(&mut self, first: &mut bool) {
        if *first {
            *first = false;
        } else {
            self.out.push_str("\n\n");
        }
    }

    fn finish(mut self) -> String {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    fn write_indent(&mut self, indent: usize) {
        self.out.push_str(&"  ".repeat(indent));
    }

    fn current_line_len(&self) -> usize {
        self.out
            .rsplit_once('\n')
            .map_or(self.out.len(), |(_, line)| line.len())
    }

    fn empty_like(&self) -> Self {
        Self {
            cst: self.cst,
            source: self.source,
            out: String::new(),
            indent: self.indent,
        }
    }

    fn node(&self, node: NodeRef) -> Node {
        self.cst.get(node)
    }

    pub fn rule(&self, node: NodeRef) -> Option<Rule> {
        match self.node(node) {
            Node::Rule(rule, _) => Some(rule),
            Node::Token(..) => None,
        }
    }

    pub fn token(&self, node: NodeRef) -> Option<Token> {
        match self.node(node) {
            Node::Rule(..) => None,
            Node::Token(token, _) => Some(token),
        }
    }

    /// Dispatches `node` to the entity owning its rule (see
    /// `entities::format_rule`). Returns false for token nodes.
    pub fn format_child(&mut self, node: NodeRef) -> bool {
        let Some(rule) = self.rule(node) else {
            return false;
        };
        format_rule(self, rule, node)
    }

    pub fn children(&self, node: NodeRef) -> Vec<NodeRef> {
        self.cst.children(node).collect()
    }

    pub fn direct_rule(&self, node: NodeRef, target: Rule) -> Option<NodeRef> {
        self.direct_rules(node, target).into_iter().next()
    }

    pub fn direct_rules(&self, node: NodeRef, target: Rule) -> Vec<NodeRef> {
        self.children(node)
            .into_iter()
            .filter(|child| self.rule(*child) == Some(target))
            .collect()
    }

    pub fn direct_token_text(&self, node: NodeRef, target: Token) -> Option<String> {
        self.direct_token_texts(node, target).into_iter().next()
    }

    pub fn direct_token_texts(&self, node: NodeRef, target: Token) -> Vec<String> {
        self.children(node)
            .into_iter()
            .filter(|child| self.token(*child) == Some(target))
            .map(|child| self.node_text(child))
            .collect()
    }

    pub fn node_text(&self, node: NodeRef) -> String {
        let span = self.node_span(node);
        self.source[span.start..span.end].to_string()
    }

    pub fn write_node_text(&mut self, node: NodeRef) {
        let text = self.node_text(node);
        self.out.push_str(&text);
    }

    pub fn write_str(&mut self, text: &str) {
        self.out.push_str(text);
    }

    pub fn direct_qualified_name_text(&self, node: NodeRef) -> Option<String> {
        let name = self.direct_rule(node, Rule::QualifiedName)?;
        self.qualified_name_text(name)
    }

    fn qualified_name_text(&self, name: NodeRef) -> Option<String> {
        let parts = self.direct_token_texts(name, Token::Name);
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("::"))
        }
    }

    pub fn direct_relation_ref_text(&self, node: NodeRef) -> Option<String> {
        let relation = self.direct_rule(node, Rule::RelationRef)?;
        let qualified = self.direct_qualified_name_text(relation)?;
        let selector = self
            .children(relation)
            .into_iter()
            .skip_while(|child| self.token(*child) != Some(Token::Arrow))
            .skip(1)
            .find_map(|child| (self.token(child) == Some(Token::Name)).then(|| self.node_text(child)));
        selector.map_or(Some(qualified.clone()), |selector| {
            Some(format!("{qualified}->{selector}"))
        })
    }

    pub fn direct_value_rule(&self, node: NodeRef) -> Option<NodeRef> {
        self.children(node).into_iter().find(|child| {
            matches!(
                self.rule(*child),
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

    fn direct_operator(&self, node: NodeRef) -> Option<NodeRef> {
        self.children(node).into_iter().find(|child| {
            if matches!(
                self.rule(*child),
                Some(Rule::BinaryOperator | Rule::OperatorVariable)
            ) {
                return true;
            }
            matches!(
                self.token(*child),
                Some(
                    Token::Eq
                        | Token::Ne
                        | Token::Gt
                        | Token::Ge
                        | Token::Lt
                        | Token::Le
                        | Token::Like
                        | Token::And
                        | Token::Or
                )
            )
        })
    }

    fn selection_has_comma(&self, node: NodeRef) -> bool {
        self.children(node)
            .into_iter()
            .any(|child| self.token(child) == Some(Token::Comma))
    }

    fn text_between(&self, left: NodeRef, right: NodeRef) -> String {
        let left_end = self.node_span(left).end;
        let right_start = self.node_span(right).start;
        if left_end > right_start {
            return String::new();
        }
        self.source[left_end..right_start].to_string()
    }

    pub fn node_span(&self, node: NodeRef) -> Span {
        Span::from(self.cst.span(node))
    }
}
