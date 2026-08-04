use crate::frontend::lexer::{Token, tokenize};
use codespan_reporting::diagnostic::Label;

pub type Diagnostic = codespan_reporting::diagnostic::Diagnostic<()>;

include!("./generated.rs");

impl<'a> ParserCallbacks<'a> for Parser<'a> {
    type Diagnostic = Diagnostic;
    type Context = ();

    fn create_tokens(
        _context: &mut Self::Context,
        source: &'a str,
        diags: &mut Vec<Self::Diagnostic>,
    ) -> (Vec<Token>, Vec<Span>) {
        tokenize(source, diags)
    }
    fn create_diagnostic(&self, span: Span, message: String) -> Self::Diagnostic {
        Diagnostic::error()
            .with_message(message)
            .with_label(Label::primary((), span))
    }
    fn predicate_decl_1(&self) -> bool {
        let peek = self.peek(1);
        peek == Token::Colon || (peek == Token::Hat && self.peek(2) == Token::Colon)
    }
}

#[cfg(test)]
mod tests {
    use super::Parser;

    #[test]
    fn advancing_recovery_at_end_of_input_is_a_no_op() {
        let mut diagnostics = Vec::new();
        let mut parser = Parser::new("start", &mut diagnostics);
        parser.init_skip();
        parser.advance(false, &mut diagnostics);
        let node_count = parser.cst.data.nodes.len();

        parser.advance(true, &mut diagnostics);

        assert_eq!(parser.pos, parser.tokens.len());
        assert_eq!(parser.cst.data.nodes.len(), node_count);
        for index in 0..node_count {
            let _ = parser.cst.data.span(super::NodeRef(index));
        }
    }
}
