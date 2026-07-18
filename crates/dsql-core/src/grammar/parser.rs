use super::lexer::{Token, tokenize};

// TODO: change if codespan_reporting is not used
use codespan_reporting::diagnostic::Label;
pub type Diagnostic = codespan_reporting::diagnostic::Diagnostic<()>;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

fn is_name_token(token: Token) -> bool {
    matches!(
        token,
        Token::Name
            | Token::Not
            | Token::In
            | Token::Is
            | Token::Exists
            | Token::Filter
            | Token::Condition
            | Token::Apply
            | Token::When
            | Token::Field
    )
}

fn starts_expression(token: Token) -> bool {
    is_name_token(token)
        || matches!(
            token,
            Token::String
                | Token::Number
                | Token::True
                | Token::False
                | Token::Null
                | Token::LBracket
                | Token::Dot
                | Token::DotDot
                | Token::Tilde
                | Token::Dollar
                | Token::DollarDollar
                | Token::LPar
        )
}

fn current_follows_previous_on_same_line(parser: &Parser<'_>) -> bool {
    let current_start = parser.span().start;
    let previous_end = parser
        .tokens
        .iter()
        .enumerate()
        .take(parser.pos)
        .rev()
        .find(|(_, token)| !Parser::is_skipped(**token))
        .and_then(|(index, _)| parser.cst.data.spans.get(index))
        .map_or(current_start, |span| span.end);
    !parser.cst.source[previous_end..current_start]
        .chars()
        .any(|character| matches!(character, '\n' | '\r'))
}

/// A directive can follow either a fragment spread or a relation source.
/// Only a following selection set commits the ellipsis form to flattening.
fn directives_end_in_selection_set(parser: &Parser<'_>) -> bool {
    let mut lookahead = 2;
    while parser.peek(lookahead) == Token::At {
        lookahead += 1;
        if parser.peek(lookahead) == Token::Dot {
            lookahead += 1;
        }
        if !is_name_token(parser.peek(lookahead)) {
            return false;
        }
        lookahead += 1;
        if parser.peek(lookahead) == Token::Dot {
            lookahead += 1;
            if !is_name_token(parser.peek(lookahead)) {
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

/// Whether the scoped path at the current token is followed by a predicate
/// aggregate pipe. The grammar keeps plain paths and aggregate values as
/// separate CST rules, so lowering does not have to reinterpret token tails.
fn scoped_path_ends_in_pipe(parser: &Parser<'_>) -> bool {
    if !matches!(parser.current, Token::Dot | Token::DotDot | Token::Tilde) {
        return false;
    }
    let mut lookahead = 1;
    loop {
        if !is_name_token(parser.peek(lookahead)) {
            return false;
        }
        lookahead += 1;
        if parser.peek(lookahead) == Token::ColonColon {
            lookahead += 1;
            if !is_name_token(parser.peek(lookahead)) {
                return false;
            }
            lookahead += 1;
        }
        if parser.peek(lookahead) == Token::Arrow {
            lookahead += 1;
            if !is_name_token(parser.peek(lookahead)) {
                return false;
            }
            lookahead += 1;
        }
        if parser.peek(lookahead) != Token::Dot {
            return parser.peek(lookahead) == Token::Pipe;
        }
        lookahead += 1;
    }
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

    fn predicate_apply_rule_1(&self) -> bool {
        self.current == Token::Where && current_follows_previous_on_same_line(self)
    }

    fn predicate_field_rule_1(&self) -> bool {
        self.current == Token::Comma
    }

    fn predicate_order_by_clause_1(&self) -> bool {
        is_name_token(self.peek(1))
    }

    fn predicate_operator_variable_1(&self) -> bool {
        matches!(
            self.peek(1),
            Token::Eq | Token::Ne | Token::Gt | Token::Ge | Token::Lt | Token::Le | Token::Like
        )
    }

    fn predicate_collection_literal_1(&self) -> bool {
        starts_expression(self.peek(1))
    }

    fn predicate_qualified_name_1(&self) -> bool {
        is_name_token(self.peek(1))
    }

    fn predicate_directive_1(&self) -> bool {
        matches!(self.current, Token::LPar)
    }

    fn predicate_directive_2(&self) -> bool {
        is_name_token(self.peek(1))
    }

    fn predicate_pipe_transform_1(&self) -> bool {
        is_name_token(self.peek(1))
            || matches!(self.peek(1), Token::Dot | Token::DotDot | Token::Tilde)
    }

    fn predicate_aggregate_field_1(&self) -> bool {
        is_name_token(self.peek(1))
    }

    fn predicate_expr_1(&self) -> bool {
        matches!(self.peek(1), Token::Dot | Token::DotDot | Token::Tilde)
            || is_name_token(self.peek(1))
    }

    fn predicate_expr_2(&self) -> bool {
        starts_expression(self.peek(1))
    }

    fn predicate_expr_3(&self) -> bool {
        scoped_path_ends_in_pipe(self)
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
