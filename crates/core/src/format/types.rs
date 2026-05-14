use crate::syntax::Diagnostic;
use facet::Facet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum FormatConfidence {
    Full,
    Partial,
    PreserveOriginal,
}

#[derive(Clone, Debug, Facet)]
pub struct FormattedText {
    pub text: String,
    pub confidence: FormatConfidence,
    pub diagnostics: Vec<Diagnostic>,
}
