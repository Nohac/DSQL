use super::lexer::{Token, tokenize};

// TODO: change if codespan_reporting is not used
use codespan_reporting::diagnostic::Label;
pub type Diagnostic = codespan_reporting::diagnostic::Diagnostic<()>;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

/// A directive can follow either a fragment spread or a relation source.
/// Only a following selection set commits the ellipsis form to flattening.
fn directives_end_in_selection_set(parser: &Parser<'_>) -> bool {
    let mut lookahead = 2;
    while parser.peek(lookahead) == Token::At {
        lookahead += 1;
        if parser.peek(lookahead) == Token::Dot {
            lookahead += 1;
        }
        if parser.peek(lookahead) != Token::Name {
            return false;
        }
        lookahead += 1;
        if parser.peek(lookahead) == Token::Dot {
            lookahead += 1;
            if parser.peek(lookahead) != Token::Name {
                return false;
            }
            lookahead += 1;
        }
        if parser.peek(lookahead) == Token::LPar {
            let mut depth = 1;
            lookahead += 1;
            while depth > 0 {
                match parser.peek(lookahead) {
                    Token::LPar => depth += 1,
                    Token::RPar => depth -= 1,
                    Token::EOF => return false,
                    _ => {}
                }
                lookahead += 1;
            }
        }
    }
    parser.peek(lookahead) == Token::LBrace
}

impl<'a> ParserCallbacks<'a> for Parser<'a> {
    type Diagnostic = Diagnostic;
    type Context = (); // TODO: add context information to the parser if required

    fn create_tokens(
        _context: &mut Self::Context,
        source: &'a str,
        diags: &mut Vec<Self::Diagnostic>,
    ) -> (Vec<Token>, Vec<Span>) {
        tokenize(source, diags)
    }
    fn create_diagnostic(&self, span: Span, message: String) -> Self::Diagnostic {
        Self::Diagnostic::error()
            .with_message(message)
            .with_label(Label::primary((), span))
    }
    fn predicate_order_by_clause_1(&self) -> bool {
        matches!(self.peek(1), Token::Name)
    }

    fn predicate_operator_variable_1(&self) -> bool {
        matches!(
            self.peek(1),
            Token::Eq | Token::Ne | Token::Gt | Token::Ge | Token::Lt | Token::Le | Token::Like
        )
    }

    fn predicate_qualified_name_1(&self) -> bool {
        matches!(self.peek(1), Token::Name)
    }

    fn predicate_directive_1(&self) -> bool {
        matches!(self.current, Token::LPar)
    }

    fn predicate_directive_2(&self) -> bool {
        matches!(self.peek(1), Token::Name)
    }

    fn predicate_pipe_transform_1(&self) -> bool {
        matches!(
            self.peek(1),
            Token::Name | Token::Dot | Token::DotDot | Token::Tilde
        )
    }

    fn predicate_aggregate_field_1(&self) -> bool {
        matches!(self.peek(1), Token::Name)
    }

    fn predicate_selection_1(&self) -> bool {
        !matches!(self.current, Token::Ellipsis)
            || matches!(
                self.peek(2),
                Token::ColonColon | Token::Arrow | Token::LPar | Token::LBrace | Token::Pipe
            )
            || (self.peek(2) == Token::At && directives_end_in_selection_set(self))
    }
}
