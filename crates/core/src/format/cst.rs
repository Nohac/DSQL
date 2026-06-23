use super::{FormatConfidence, FormatDiagnostic, FormattedText};
use crate::language::{atoms::directive::DirectiveAtom, stages::FormatsAtom};
use crate::syntax::{CstKind, ParseResult, SyntaxNode, SyntaxRule, SyntaxToken, TextRange};

const DEFAULT_FORMAT_LINE_WIDTH: usize = 100;

pub fn format_file(parse: &ParseResult) -> FormattedText {
    if !parse.diagnostics.is_empty() {
        return FormattedText {
            text: original_text(parse),
            confidence: FormatConfidence::PreserveOriginal,
            diagnostics: vec![FormatDiagnostic {
                range: parse
                    .diagnostics
                    .first()
                    .map_or(TextRange::default(), |diag| diag.range),
            }],
        };
    }

    let mut formatter = CstFormatter::new(parse);
    formatter.document(0);
    FormattedText {
        text: formatter.finish(),
        confidence: FormatConfidence::Full,
        diagnostics: Vec::new(),
    }
}

fn original_text(parse: &ParseResult) -> String {
    parse
        .source
        .text(TextRange::new(0, parse.source.len_bytes()))
        .into_owned()
}

pub(crate) struct CstFormatter<'a> {
    parse: &'a ParseResult,
    out: String,
    indent: usize,
}

impl<'a> CstFormatter<'a> {
    fn new(parse: &'a ParseResult) -> Self {
        Self {
            parse,
            out: String::new(),
            indent: 0,
        }
    }

    fn document(&mut self, node: usize) {
        let mut first = true;
        for child in self.node(node).children.clone() {
            match (self.rule(child), self.token(child)) {
                (_, Some(SyntaxToken::Comment)) => {
                    self.blank_between_definitions(&mut first);
                    self.out.push_str(&self.text(self.node(child).range));
                }
                (Some(SyntaxRule::QueryDef), _) => {
                    self.blank_between_definitions(&mut first);
                    self.query(child);
                }
                (Some(SyntaxRule::FragmentDef), _) => {
                    self.blank_between_definitions(&mut first);
                    self.fragment(child);
                }
                _ => {}
            }
        }
    }

    fn query(&mut self, node: usize) {
        self.out.push_str("query");
        if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
            self.out.push(' ');
            self.out.push_str(&name);
        }
        if let Some(selection_set) = self.direct_rule(node, SyntaxRule::SelectionSet) {
            self.selection_set(selection_set);
        }
    }

    fn fragment(&mut self, node: usize) {
        self.out.push_str("fragment");
        let names = self.direct_token_texts(node, SyntaxToken::Name);
        if let Some(name) = names.first() {
            self.out.push(' ');
            self.out.push_str(name);
        }
        self.out.push_str(" on");
        if let Some(on) = self.direct_qualified_name_text(node) {
            self.out.push(' ');
            self.out.push_str(&on);
        }
        if let Some(selection_set) = self.direct_rule(node, SyntaxRule::SelectionSet) {
            self.selection_set(selection_set);
        }
    }

    fn selection_set(&mut self, node: usize) {
        self.out.push_str(" {\n");
        self.indent += 1;
        let children = self.node(node).children.clone();
        let mut idx = 0;
        while idx < children.len() {
            let child = children[idx];
            match (self.rule(child), self.token(child)) {
                (Some(SyntaxRule::Selection), _) => {
                    self.indent();
                    self.selection(child);
                    let mut current = child;
                    while self.selection_has_comma(current) {
                        let Some(next_idx) = next_selection_index(&children, idx + 1, |node| {
                            self.rule(node) == Some(SyntaxRule::Selection)
                        }) else {
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
                (_, Some(SyntaxToken::Comment)) => {
                    self.indent();
                    self.out.push_str(&self.text(self.node(child).range));
                    self.out.push('\n');
                }
                _ => {}
            }
            idx += 1;
        }
        self.indent = self.indent.saturating_sub(1);
        self.indent();
        self.out.push('}');
    }

    fn selection(&mut self, node: usize) {
        if let Some(field) = self.direct_rule(node, SyntaxRule::FieldSelection) {
            self.field_selection(field);
        } else if let Some(spread) = self.direct_rule(node, SyntaxRule::FragmentSpread) {
            self.fragment_spread(spread);
        }
    }

    fn fragment_spread(&mut self, node: usize) {
        if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
            self.out.push_str("...");
            self.out.push_str(&name);
        }
        for directive in self.direct_rules(node, SyntaxRule::Directive) {
            <Self as FormatsAtom<DirectiveAtom>>::format(self, directive);
        }
    }

    fn field_selection(&mut self, node: usize) {
        let first = self.direct_relation_ref_text(node);
        let tail = self.direct_rule(node, SyntaxRule::FieldSelectionTail);
        let (alias, name, suffix) = if let Some(tail) = tail {
            let tail_name = self.direct_relation_ref_text(tail);
            if tail_name.is_some() {
                (
                    first,
                    tail_name,
                    self.direct_rule(tail, SyntaxRule::FieldSuffix),
                )
            } else {
                (None, first, self.direct_rule(tail, SyntaxRule::FieldSuffix))
            }
        } else {
            (None, first, None)
        };
        if let Some(alias) = alias {
            self.out.push_str(&alias);
            self.out.push_str(": ");
        }
        if let Some(name) = name {
            self.out.push_str(&name);
        }
        if let Some(suffix) = suffix {
            self.field_suffix(suffix);
        }
    }

    fn field_suffix(&mut self, node: usize) {
        if let Some(clauses) = self.direct_rule(node, SyntaxRule::ClauseList) {
            self.clause_list(clauses);
        }
        for directive in self.direct_rules(node, SyntaxRule::Directive) {
            <Self as FormatsAtom<DirectiveAtom>>::format(self, directive);
        }
        if let Some(selection_set) = self.direct_rule(node, SyntaxRule::SelectionSet) {
            self.selection_set(selection_set);
        }
    }

    fn clause_list(&mut self, node: usize) {
        let clauses = self.direct_rules(node, SyntaxRule::Clause);
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

    fn inline_clause_list_text(&self, clauses: &[usize]) -> String {
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

    fn clause_text(&self, clause: usize) -> String {
        let mut formatter = self.empty_like();
        formatter.clause(clause);
        formatter.out
    }

    fn clause_list_multiline(&mut self, clauses: &[usize], preserve_user_breaks: bool) {
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

    fn complex_where_clause(&self, clauses: &[usize]) -> Option<(usize, Vec<usize>)> {
        let first = *clauses.first()?;
        let where_clause = self.direct_rule(first, SyntaxRule::WhereClause)?;
        let value = self.direct_value_rule(where_clause)?;
        self.is_complex_predicate(value)
            .then(|| (where_clause, clauses[1..].to_vec()))
    }

    fn clause_list_has_linebreaks(&self, clauses: &[usize]) -> bool {
        clauses.windows(2).any(|pair| {
            let [left, right] = pair else {
                return false;
            };
            self.text_between(*left, *right).contains('\n')
        })
    }

    fn where_clause_multiline(&mut self, node: usize, continuation_indent: usize) {
        self.out.push_str("where ");
        if let Some(value) = self.direct_value_rule(node) {
            self.expr_multiline(value, continuation_indent);
        }
    }

    fn is_where_clause(&self, node: usize) -> bool {
        self.direct_rule(node, SyntaxRule::WhereClause).is_some()
    }

    fn clause(&mut self, node: usize) {
        if let Some(where_clause) = self.direct_rule(node, SyntaxRule::WhereClause) {
            self.out.push_str("where ");
            if let Some(value) = self.direct_value_rule(where_clause) {
                self.expr(value);
            }
        } else if let Some(order_by) = self.direct_rule(node, SyntaxRule::OrderByClause) {
            self.out.push_str("order by ");
            for (idx, item) in self
                .direct_rules(order_by, SyntaxRule::OrderItem)
                .into_iter()
                .enumerate()
            {
                if idx > 0 {
                    self.out.push_str(", ");
                }
                self.order_item(item);
            }
        } else if let Some(limit) = self.direct_rule(node, SyntaxRule::LimitClause) {
            self.out.push_str("limit ");
            if let Some(value) = self.direct_value_rule(limit) {
                self.expr(value);
            }
        } else if let Some(offset) = self.direct_rule(node, SyntaxRule::OffsetClause) {
            self.out.push_str("offset ");
            if let Some(value) = self.direct_value_rule(offset) {
                self.expr(value);
            }
        }
    }

    fn order_item(&mut self, node: usize) {
        if let Some(name) = self.direct_qualified_name_text(node) {
            self.out.push_str(&name);
        }
        if let Some(direction) = self.direct_rule(node, SyntaxRule::SortDirection) {
            self.out.push(' ');
            self.out.push_str(&self.text(self.node(direction).range));
        }
    }

    fn expr(&mut self, node: usize) {
        match self.rule(node) {
            Some(SyntaxRule::BinaryExpr) => self.binary_expr(node),
            Some(SyntaxRule::Literal) => self.literal(node),
            Some(SyntaxRule::Expr) => {
                let grouped = self
                    .node(node)
                    .children
                    .iter()
                    .any(|child| self.token(*child) == Some(SyntaxToken::LPar));
                if grouped {
                    self.out.push('(');
                }
                if let Some(binary) = self.direct_rule(node, SyntaxRule::BinaryExpr) {
                    self.binary_expr(binary);
                } else if let Some(literal) = self.direct_rule(node, SyntaxRule::Literal) {
                    self.literal(literal);
                } else if let Some(path) = self.direct_rule(node, SyntaxRule::ScopedPath) {
                    self.scoped_path(path);
                } else if let Some(variable) = self.direct_rule(node, SyntaxRule::ValueVariable) {
                    self.value_variable(variable);
                } else if let Some(name) = self.direct_qualified_name_text(node) {
                    self.out.push_str(&name);
                } else if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
                    self.out.push_str(&name);
                }
                if grouped {
                    self.out.push(')');
                }
            }
            Some(SyntaxRule::QualifiedName) => {
                if let Some(name) = self.direct_qualified_name_text(node) {
                    self.out.push_str(&name);
                }
            }
            Some(SyntaxRule::ScopedPath) => self.scoped_path(node),
            Some(SyntaxRule::ValueVariable) => self.value_variable(node),
            _ => {}
        }
    }

    fn scoped_path(&mut self, node: usize) {
        self.out.push_str(&self.text(self.node(node).range));
    }

    fn value_variable(&mut self, node: usize) {
        self.out.push_str(&self.text(self.node(node).range));
    }

    fn expr_multiline(&mut self, node: usize, line_indent: usize) {
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

        if self.rule(node) == Some(SyntaxRule::BinaryExpr)
            && let Some(op) = self.direct_operator(node)
            && matches!(self.token(op), Some(SyntaxToken::And | SyntaxToken::Or))
        {
            let exprs = self.direct_expr_operands(node);
            if let Some(left) = exprs.first().copied() {
                self.expr_multiline(left, line_indent);
            }
            if let Some(right) = exprs.get(1).copied() {
                self.out.push('\n');
                self.write_indent(line_indent);
                self.out.push_str(&self.text(self.node(op).range));
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

    fn binary_expr(&mut self, node: usize) {
        let exprs = self.direct_expr_operands(node);
        if let Some(left) = exprs.first().copied() {
            self.expr(left);
        }
        if let Some(op) = self.direct_operator(node) {
            self.out.push(' ');
            self.out.push_str(&self.text(self.node(op).range));
            self.out.push(' ');
        }
        if let Some(right) = exprs.get(1).copied() {
            self.expr(right);
        }
    }

    fn direct_expr_operands(&self, node: usize) -> Vec<usize> {
        self.node(node)
            .children
            .iter()
            .copied()
            .filter(|child| {
                matches!(
                    self.rule(*child),
                    Some(SyntaxRule::Expr | SyntaxRule::BinaryExpr)
                )
            })
            .collect()
    }

    fn is_complex_predicate(&self, node: usize) -> bool {
        self.is_grouped_expr(node)
            || self.node(node).children.iter().any(|child| {
                matches!(self.token(*child), Some(SyntaxToken::And | SyntaxToken::Or))
                    || self.is_complex_predicate(*child)
            })
    }

    fn is_grouped_expr(&self, node: usize) -> bool {
        self.rule(node) == Some(SyntaxRule::Expr)
            && self
                .node(node)
                .children
                .iter()
                .any(|child| self.token(*child) == Some(SyntaxToken::LPar))
    }

    fn literal(&mut self, node: usize) {
        if let Some(token) = self.token_children(node).into_iter().find(|token| {
            !matches!(
                self.token(*token),
                Some(SyntaxToken::Whitespace | SyntaxToken::Comment)
            )
        }) {
            self.out.push_str(&self.text(self.node(token).range));
        }
    }

    fn blank_between_definitions(&mut self, first: &mut bool) {
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

    fn indent(&mut self) {
        self.write_indent(self.indent);
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
            parse: self.parse,
            out: String::new(),
            indent: self.indent,
        }
    }

    fn text(&self, range: TextRange) -> String {
        self.parse.source.text(range).into_owned()
    }

    fn node(&self, node: usize) -> &SyntaxNode {
        &self.parse.tree.nodes[node]
    }

    fn rule(&self, node: usize) -> Option<SyntaxRule> {
        match self.node(node).cst_kind {
            CstKind::Rule(rule) => Some(rule),
            CstKind::Token(_) => None,
        }
    }

    fn token(&self, node: usize) -> Option<SyntaxToken> {
        match self.node(node).cst_kind {
            CstKind::Rule(_) => None,
            CstKind::Token(token) => Some(token),
        }
    }

    fn token_children(&self, node: usize) -> Vec<usize> {
        self.node(node)
            .children
            .iter()
            .copied()
            .filter(|child| matches!(self.node(*child).cst_kind, CstKind::Token(_)))
            .collect()
    }

    fn direct_rule(&self, node: usize, target: SyntaxRule) -> Option<usize> {
        self.direct_rules(node, target).into_iter().next()
    }

    fn direct_rules(&self, node: usize, target: SyntaxRule) -> Vec<usize> {
        self.node(node)
            .children
            .iter()
            .copied()
            .filter(|child| self.rule(*child) == Some(target))
            .collect()
    }

    pub(crate) fn direct_token_text(&self, node: usize, target: SyntaxToken) -> Option<String> {
        self.direct_token_texts(node, target).into_iter().next()
    }

    pub(crate) fn write_str(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn direct_token_texts(&self, node: usize, target: SyntaxToken) -> Vec<String> {
        self.node(node)
            .children
            .iter()
            .copied()
            .filter(|child| self.token(*child) == Some(target))
            .map(|child| self.text(self.node(child).range))
            .collect()
    }

    fn direct_qualified_name_text(&self, node: usize) -> Option<String> {
        let name = self.direct_rule(node, SyntaxRule::QualifiedName)?;
        let parts = self.direct_token_texts(name, SyntaxToken::Name);
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("::"))
        }
    }

    fn direct_relation_ref_text(&self, node: usize) -> Option<String> {
        let relation = self.direct_rule(node, SyntaxRule::RelationRef)?;
        let qualified = self.direct_qualified_name_text(relation)?;
        let selector = self
            .node(relation)
            .children
            .iter()
            .copied()
            .skip_while(|child| self.token(*child) != Some(SyntaxToken::Arrow))
            .skip(1)
            .find_map(|child| {
                (self.token(child) == Some(SyntaxToken::Name))
                    .then(|| self.text(self.node(child).range))
            });
        selector.map_or(Some(qualified.clone()), |selector| {
            Some(format!("{qualified}->{selector}"))
        })
    }

    fn direct_value_rule(&self, node: usize) -> Option<usize> {
        self.node(node).children.iter().copied().find(|child| {
            matches!(
                self.rule(*child),
                Some(
                    SyntaxRule::Expr
                        | SyntaxRule::BinaryExpr
                        | SyntaxRule::Literal
                        | SyntaxRule::QualifiedName
                        | SyntaxRule::ScopedPath
                        | SyntaxRule::ValueVariable
                )
            )
        })
    }

    fn direct_operator(&self, node: usize) -> Option<usize> {
        self.node(node).children.iter().copied().find(|child| {
            if matches!(
                self.rule(*child),
                Some(SyntaxRule::BinaryOperator | SyntaxRule::OperatorVariable)
            ) {
                return true;
            }
            matches!(
                self.token(*child),
                Some(
                    SyntaxToken::Eq
                        | SyntaxToken::Ne
                        | SyntaxToken::Gt
                        | SyntaxToken::Ge
                        | SyntaxToken::Lt
                        | SyntaxToken::Le
                        | SyntaxToken::Like
                        | SyntaxToken::And
                        | SyntaxToken::Or
                )
            )
        })
    }

    fn selection_has_comma(&self, node: usize) -> bool {
        self.node(node)
            .children
            .iter()
            .any(|child| self.token(*child) == Some(SyntaxToken::Comma))
    }

    fn text_between(&self, left: usize, right: usize) -> String {
        let left_end = self.node(left).range.end;
        let right_start = self.node(right).range.start;
        if left_end > right_start {
            return String::new();
        }
        self.parse
            .source
            .text(TextRange::new(left_end as usize, right_start as usize))
            .into_owned()
    }
}

fn next_selection_index(
    children: &[usize],
    start: usize,
    is_selection: impl Fn(usize) -> bool,
) -> Option<usize> {
    children
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(idx, child)| is_selection(*child).then_some(idx))
}
