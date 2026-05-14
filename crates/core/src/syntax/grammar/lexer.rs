use super::parser::{Diagnostic, Span};
use codespan_reporting::diagnostic::Label;
use logos::Logos;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum LexerError {
    #[default]
    Invalid,
    // TODO: add more errors if required
}

impl LexerError {
    pub fn into_diagnostic(self, span: Span) -> Diagnostic {
        match self {
            Self::Invalid => Diagnostic::error()
                .with_message("invalid token")
                .with_label(Label::primary((), span)),
        }
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Logos, Debug, PartialEq, Copy, Clone)]
#[logos(error = LexerError)]
pub enum Token {
    EOF,
    #[token("query")]
    Query,
    #[token("fragment")]
    Fragment,
    #[token("on")]
    On,
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("null")]
    Null,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("(")]
    LPar,
    #[token(")")]
    RPar,
    #[token(":")]
    Colon,
    #[token("@")]
    At,
    #[token(",")]
    Comma,
    #[token("...")]
    Ellipsis,
    #[token(".")]
    Dot,
    #[token("==")]
    Eq,
    #[token("!=")]
    Ne,
    #[token(">")]
    Gt,
    #[token(">=")]
    Ge,
    #[token("<")]
    Lt,
    #[token("<=")]
    Le,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Name,
    #[regex(r#""([^"\\]|\\.)*""#)]
    String,
    #[regex(r"-?[0-9]+(\.[0-9]+)?")]
    Number,
    #[regex(r"[ \t\r\n]+")]
    Whitespace,
    #[regex(r"#[^\n\r]*", allow_greedy = true)]
    Comment,
    Error,
}

pub fn tokenize(source: &str, diags: &mut Vec<Diagnostic>) -> (Vec<Token>, Vec<Span>) {
    let lexer = Token::lexer(source);
    let mut tokens = vec![];
    let mut spans = vec![];

    for (token, span) in lexer.spanned() {
        match token {
            Ok(token) => {
                tokens.push(token);
            }
            Err(err) => {
                diags.push(err.into_diagnostic(span.clone()));
                tokens.push(Token::Error);
            }
        }
        spans.push(span);
    }
    (tokens, spans)
}
