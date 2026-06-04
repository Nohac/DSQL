use crate::{
    format::FormatDiagnostic,
    lint::LintDiagnostic,
    plan::PlanDiagnostic,
    semantic::{CheckDiagnostic, LowerDiagnostic},
    syntax::{Diagnostic, DiagnosticCode, DiagnosticSource, Severity, TextRange},
};
use facet::Facet;

pub trait DsqlDiagnostic: std::error::Error + miette::Diagnostic {
    fn range(&self) -> TextRange;
    fn severity(&self) -> Severity;
    fn code(&self) -> DiagnosticCode;
    fn source(&self) -> DiagnosticSource;

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

impl DsqlDiagnostic for CompilerDiagnostic {
    fn range(&self) -> TextRange {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => diagnostic.range(),
            CompilerDiagnostic::Lower(diagnostic) => diagnostic.range(),
            CompilerDiagnostic::Check(diagnostic) => diagnostic.range(),
            CompilerDiagnostic::Lint(diagnostic) => diagnostic.range(),
            CompilerDiagnostic::Plan(diagnostic) => diagnostic.range(),
            CompilerDiagnostic::Format(diagnostic) => diagnostic.range(),
        }
    }

    fn severity(&self) -> Severity {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => diagnostic.severity(),
            CompilerDiagnostic::Lower(diagnostic) => diagnostic.severity(),
            CompilerDiagnostic::Check(diagnostic) => diagnostic.severity(),
            CompilerDiagnostic::Lint(diagnostic) => diagnostic.severity(),
            CompilerDiagnostic::Plan(diagnostic) => diagnostic.severity(),
            CompilerDiagnostic::Format(diagnostic) => diagnostic.severity(),
        }
    }

    fn code(&self) -> DiagnosticCode {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => diagnostic.code(),
            CompilerDiagnostic::Lower(diagnostic) => diagnostic.code(),
            CompilerDiagnostic::Check(diagnostic) => diagnostic.code(),
            CompilerDiagnostic::Lint(diagnostic) => diagnostic.code(),
            CompilerDiagnostic::Plan(diagnostic) => diagnostic.code(),
            CompilerDiagnostic::Format(diagnostic) => diagnostic.code(),
        }
    }

    fn source(&self) -> DiagnosticSource {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => diagnostic.source(),
            CompilerDiagnostic::Lower(diagnostic) => diagnostic.source(),
            CompilerDiagnostic::Check(diagnostic) => diagnostic.source(),
            CompilerDiagnostic::Lint(diagnostic) => diagnostic.source(),
            CompilerDiagnostic::Plan(diagnostic) => diagnostic.source(),
            CompilerDiagnostic::Format(diagnostic) => diagnostic.source(),
        }
    }
}

impl std::fmt::Display for CompilerDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => diagnostic.fmt(f),
            CompilerDiagnostic::Lower(diagnostic) => diagnostic.fmt(f),
            CompilerDiagnostic::Check(diagnostic) => diagnostic.fmt(f),
            CompilerDiagnostic::Lint(diagnostic) => diagnostic.fmt(f),
            CompilerDiagnostic::Plan(diagnostic) => diagnostic.fmt(f),
            CompilerDiagnostic::Format(diagnostic) => diagnostic.fmt(f),
        }
    }
}

impl std::error::Error for CompilerDiagnostic {}

impl miette::Diagnostic for CompilerDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => miette::Diagnostic::code(diagnostic),
            CompilerDiagnostic::Lower(diagnostic) => miette::Diagnostic::code(diagnostic),
            CompilerDiagnostic::Check(diagnostic) => miette::Diagnostic::code(diagnostic),
            CompilerDiagnostic::Lint(diagnostic) => miette::Diagnostic::code(diagnostic),
            CompilerDiagnostic::Plan(diagnostic) => miette::Diagnostic::code(diagnostic),
            CompilerDiagnostic::Format(diagnostic) => miette::Diagnostic::code(diagnostic),
        }
    }

    fn severity(&self) -> Option<miette::Severity> {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => miette::Diagnostic::severity(diagnostic),
            CompilerDiagnostic::Lower(diagnostic) => miette::Diagnostic::severity(diagnostic),
            CompilerDiagnostic::Check(diagnostic) => miette::Diagnostic::severity(diagnostic),
            CompilerDiagnostic::Lint(diagnostic) => miette::Diagnostic::severity(diagnostic),
            CompilerDiagnostic::Plan(diagnostic) => miette::Diagnostic::severity(diagnostic),
            CompilerDiagnostic::Format(diagnostic) => miette::Diagnostic::severity(diagnostic),
        }
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        match self {
            CompilerDiagnostic::Parse(diagnostic) => miette::Diagnostic::labels(diagnostic),
            CompilerDiagnostic::Lower(diagnostic) => miette::Diagnostic::labels(diagnostic),
            CompilerDiagnostic::Check(diagnostic) => miette::Diagnostic::labels(diagnostic),
            CompilerDiagnostic::Lint(diagnostic) => miette::Diagnostic::labels(diagnostic),
            CompilerDiagnostic::Plan(diagnostic) => miette::Diagnostic::labels(diagnostic),
            CompilerDiagnostic::Format(diagnostic) => miette::Diagnostic::labels(diagnostic),
        }
    }
}

impl From<Diagnostic> for CompilerDiagnostic {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Parse(diagnostic)
    }
}

impl From<LowerDiagnostic> for CompilerDiagnostic {
    fn from(diagnostic: LowerDiagnostic) -> Self {
        Self::Lower(diagnostic)
    }
}

impl From<CheckDiagnostic> for CompilerDiagnostic {
    fn from(diagnostic: CheckDiagnostic) -> Self {
        Self::Check(diagnostic)
    }
}

impl From<LintDiagnostic> for CompilerDiagnostic {
    fn from(diagnostic: LintDiagnostic) -> Self {
        Self::Lint(diagnostic)
    }
}

impl From<PlanDiagnostic> for CompilerDiagnostic {
    fn from(diagnostic: PlanDiagnostic) -> Self {
        Self::Plan(diagnostic)
    }
}

impl From<FormatDiagnostic> for CompilerDiagnostic {
    fn from(diagnostic: FormatDiagnostic) -> Self {
        Self::Format(diagnostic)
    }
}
