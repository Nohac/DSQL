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
    #[token("where")]
    Where,
    #[token("order")]
    Order,
    #[token("by")]
    By,
    #[token("limit")]
    Limit,
    #[token("offset")]
    Offset,
    #[token("asc")]
    Asc,
    #[token("desc")]
    Desc,
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("null")]
    Null,
    #[token("like")]
    Like,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("(")]
    LPar,
    #[token(")")]
    RPar,
    #[token("::")]
    ColonColon,
    #[token(":")]
    Colon,
    #[token("@")]
    At,
    #[token(",")]
    Comma,
    #[token("...")]
    Ellipsis,
    #[token("..")]
    DotDot,
    #[token(".")]
    Dot,
    #[token("~")]
    Tilde,
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

impl Token {
    pub const fn completion_label(self) -> Option<&'static str> {
        match self {
            Self::Query => Some("query"),
            Self::Fragment => Some("fragment"),
            Self::On => Some("on"),
            Self::Where => Some("where"),
            Self::Order => Some("order"),
            Self::By => Some("by"),
            Self::Limit => Some("limit"),
            Self::Offset => Some("offset"),
            Self::Asc => Some("asc"),
            Self::Desc => Some("desc"),
            Self::Eq => Some("=="),
            Self::Ne => Some("!="),
            Self::Gt => Some(">"),
            Self::Ge => Some(">="),
            Self::Lt => Some("<"),
            Self::Le => Some("<="),
            Self::Like => Some("like"),
            _ => None,
        }
    }
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
