use crate::{
    format::FormatDiagnostic,
    lint::LintDiagnostic,
    plan::PlanDiagnostic,
    semantic::{CheckDiagnostic, LowerDiagnostic},
    syntax::{Diagnostic, DiagnosticCode, DiagnosticSource, Severity, TextRange},
};
use enum_dispatch::enum_dispatch;
use facet::Facet;

pub trait CompilerDiagnosticSource {
    fn extend_compiler_diagnostics(&self, diagnostics: &mut Vec<CompilerDiagnostic>);
}

#[enum_dispatch(CompilerDiagnostic)]
pub trait DsqlDiagnostic: std::error::Error + miette::Diagnostic {
    fn range(&self) -> TextRange;
    fn severity(&self) -> Severity;
    fn code(&self) -> DiagnosticCode;
    fn source(&self) -> DiagnosticSource;

    fn fmt_display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }

    fn miette_code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        miette::Diagnostic::code(self)
    }

    fn miette_severity(&self) -> Option<miette::Severity> {
        miette::Diagnostic::severity(self)
    }

    fn miette_labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        miette::Diagnostic::labels(self)
    }

    fn to_transport(&self) -> Diagnostic {
        Diagnostic {
            range: self.range(),
            severity: DsqlDiagnostic::severity(self),
            code: DsqlDiagnostic::code(self),
            source: DsqlDiagnostic::source(self),
            message: self.to_string(),
        }
    }
}

impl DsqlDiagnostic for Diagnostic {
    fn range(&self) -> TextRange {
        self.range
    }

    fn severity(&self) -> Severity {
        self.severity
    }

    fn code(&self) -> DiagnosticCode {
        self.code.clone()
    }

    fn source(&self) -> DiagnosticSource {
        self.source
    }
}

#[enum_dispatch]
#[derive(Clone, Debug, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum CompilerDiagnostic {
    Parse(Diagnostic),
    Lower(LowerDiagnostic),
    Check(CheckDiagnostic),
    Lint(LintDiagnostic),
    Plan(PlanDiagnostic),
    Format(FormatDiagnostic),
}

impl std::fmt::Display for CompilerDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_display(f)
    }
}

impl std::error::Error for CompilerDiagnostic {}

impl miette::Diagnostic for CompilerDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.miette_code()
    }

    fn severity(&self) -> Option<miette::Severity> {
        self.miette_severity()
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        self.miette_labels()
    }
}

pub fn collect_query_compiler_diagnostics(
    checked: &crate::CheckedDefinition,
    linted: &crate::LintedDefinition,
    planned: &crate::PlannedFile,
) -> Vec<CompilerDiagnostic> {
    collect_compiler_diagnostic_sources(&[checked, linted, planned])
}

pub fn collect_compiler_diagnostic_sources(
    sources: &[&dyn CompilerDiagnosticSource],
) -> Vec<CompilerDiagnostic> {
    let mut diagnostics = Vec::new();
    for source in sources {
        source.extend_compiler_diagnostics(&mut diagnostics);
    }
    sort_compiler_diagnostics(&mut diagnostics);
    diagnostics
}

pub(crate) fn extend_compiler_diagnostics<T>(
    diagnostics: &mut Vec<CompilerDiagnostic>,
    values: impl IntoIterator<Item = T>,
) where
    T: Into<CompilerDiagnostic>,
{
    diagnostics.extend(values.into_iter().map(Into::into));
}

pub fn sort_compiler_diagnostics(diagnostics: &mut [CompilerDiagnostic]) {
    diagnostics.sort_by_key(|diagnostic| {
        let range = diagnostic.range();
        (range.start, range.end)
    });
}
