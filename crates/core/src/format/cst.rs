use super::{FormatConfidence, FormattedText};
use crate::syntax::{
    CstKind, Diagnostic, DiagnosticCode, DiagnosticSource, ParseResult, Severity, SyntaxNode,
    SyntaxRule, SyntaxToken, TextRange,
};

pub fn format_file(parse: &ParseResult) -> FormattedText {
    if !parse.diagnostics.is_empty() {
        return FormattedText {
            text: original_text(parse),
            confidence: FormatConfidence::PreserveOriginal,
            diagnostics: vec![Diagnostic {
                range: parse
                    .diagnostics
                    .first()
                    .map_or(TextRange::default(), |diag| diag.range),
                severity: Severity::Error,
                code: DiagnosticCode::FormatParseError,
                message: "refusing to format a file with parse errors".to_string(),
                source: DiagnosticSource::Format,
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

struct CstFormatter<'a> {
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
        for child in self.node(node).children.clone() {
            match (self.rule(child), self.token(child)) {
                (Some(SyntaxRule::Selection), _) => {
                    self.indent();
                    self.selection(child);
                    self.out.push('\n');
                }
                (_, Some(SyntaxToken::Comment)) => {
                    self.indent();
                    self.out.push_str(&self.text(self.node(child).range));
                    self.out.push('\n');
                }
                _ => {}
            }
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
            self.directive(directive);
        }
    }

    fn field_selection(&mut self, node: usize) {
        let first = self.direct_qualified_name_text(node);
        let tail = self.direct_rule(node, SyntaxRule::FieldSelectionTail);
        let (alias, name, suffix) = if let Some(tail) = tail {
            let tail_name = self.direct_qualified_name_text(tail);
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
        if let Some(arguments) = self.direct_rule(node, SyntaxRule::ArgumentList) {
            self.argument_list(arguments);
        }
        for directive in self.direct_rules(node, SyntaxRule::Directive) {
            self.directive(directive);
        }
        if let Some(selection_set) = self.direct_rule(node, SyntaxRule::SelectionSet) {
            self.selection_set(selection_set);
        }
    }

    fn argument_list(&mut self, node: usize) {
        self.out.push('(');
        for (idx, argument) in self
            .direct_rules(node, SyntaxRule::Argument)
            .into_iter()
            .enumerate()
        {
            if idx > 0 {
                self.out.push_str(", ");
            }
            self.argument(argument);
        }
        self.out.push(')');
    }

    fn argument(&mut self, node: usize) {
        if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
            self.out.push_str(&name);
            self.out.push(' ');
        }
        if let Some(value) = self.direct_value_rule(node) {
            self.expr(value);
        }
    }

    fn directive(&mut self, node: usize) {
        if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
            self.out.push_str(" @");
            self.out.push_str(&name);
        }
    }

    fn expr(&mut self, node: usize) {
        match self.rule(node) {
            Some(SyntaxRule::BinaryExpr) => self.binary_expr(node),
            Some(SyntaxRule::Literal) => self.literal(node),
            Some(SyntaxRule::Expr) => {
                if let Some(binary) = self.direct_rule(node, SyntaxRule::BinaryExpr) {
                    self.binary_expr(binary);
                } else if let Some(literal) = self.direct_rule(node, SyntaxRule::Literal) {
                    self.literal(literal);
                } else if let Some(name) = self.direct_token_text(node, SyntaxToken::Name) {
                    self.out.push_str(&name);
                }
            }
            _ => {}
        }
    }

    fn binary_expr(&mut self, node: usize) {
        let exprs = self.direct_rules(node, SyntaxRule::Expr);
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
        self.out.push_str(&"  ".repeat(self.indent));
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

    fn direct_token_text(&self, node: usize, target: SyntaxToken) -> Option<String> {
        self.direct_token_texts(node, target).into_iter().next()
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
            Some(parts.join("."))
        }
    }

    fn direct_value_rule(&self, node: usize) -> Option<usize> {
        self.node(node).children.iter().copied().find(|child| {
            matches!(
                self.rule(*child),
                Some(SyntaxRule::Expr | SyntaxRule::BinaryExpr | SyntaxRule::Literal)
            )
        })
    }

    fn direct_operator(&self, node: usize) -> Option<usize> {
        self.node(node).children.iter().copied().find(|child| {
            matches!(
                self.token(*child),
                Some(
                    SyntaxToken::Eq
                        | SyntaxToken::Ne
                        | SyntaxToken::Gt
                        | SyntaxToken::Ge
                        | SyntaxToken::Lt
                        | SyntaxToken::Le
                )
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SourceSnapshot, parse_source};

    #[test]
    fn formats_from_cst_selection_boundaries() {
        let parsed = parse_source(SourceSnapshot::from(
            "query Users { users(where age > 18) { id name } }",
        ));
        let formatted = format_file(&parsed);
        assert!(
            formatted.diagnostics.is_empty(),
            "{:?}",
            formatted.diagnostics
        );
        assert_eq!(
            formatted.text,
            "query Users {\n  users(where age > 18) {\n    id\n    name\n  }\n}\n"
        );
    }

    #[test]
    fn preserves_cst_comment_trivia_in_selection_sets() {
        let parsed = parse_source(SourceSnapshot::from("query Users { # ids\n id }"));
        let formatted = format_file(&parsed);
        assert!(
            formatted.diagnostics.is_empty(),
            "{:?}",
            formatted.diagnostics
        );
        assert_eq!(formatted.text, "query Users {\n  # ids\n  id\n}\n");
    }
}
