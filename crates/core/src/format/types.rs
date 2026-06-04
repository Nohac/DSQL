use crate::{
    diagnostics::DsqlDiagnostic,
    syntax::{DiagnosticCode, DiagnosticSource, Severity, TextRange, source_span},
};
use facet::Facet;
use miette::LabeledSpan;
use std::fmt;

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
    pub diagnostics: Vec<FormatDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Facet, thiserror::Error)]
#[error("refusing to format a file with parse errors")]
pub struct FormatDiagnostic {
    pub range: TextRange,
}

impl miette::Diagnostic for FormatDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(format!("{:?}", DsqlDiagnostic::code(self))))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Error)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::underline(
            source_span(self.range),
        ))))
    }
}

impl DsqlDiagnostic for FormatDiagnostic {
    fn range(&self) -> TextRange {
        self.range
    }

    fn severity(&self) -> Severity {
        Severity::Error
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::FormatParseError
    }

    fn source(&self) -> DiagnosticSource {
        DiagnosticSource::Format
    }
}
