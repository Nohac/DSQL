use dsql_core::{
    Catalog, CompilerDiagnosticSource, Diagnostic, DsqlDiagnostic, Interner, SourceSnapshot,
    check_file_with_catalog, collect_compiler_diagnostic_sources, lint_file_with_catalog,
    lower_file, parse_source, plan_file_with_catalog,
};

use crate::db::AnalysisResult;

pub fn analyze_source(source: SourceSnapshot) -> AnalysisResult {
    let parse = parse_source(source);
    let mut interner = Interner::default();
    let lower = lower_file(&parse.source_file, &mut interner);
    let check = check_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    let lint = lint_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    let plan = plan_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    AnalysisResult {
        parse,
        lower,
        check,
        lint,
        plan,
    }
}

pub fn collect_diagnostics(analysis: &AnalysisResult) -> Vec<Diagnostic> {
    collect_diagnostics_parts(&[
        &analysis.parse,
        &analysis.lower,
        &analysis.check,
        &analysis.lint,
        &analysis.plan,
    ])
}

pub(crate) fn collect_diagnostics_parts(
    sources: &[&dyn CompilerDiagnosticSource],
) -> Vec<Diagnostic> {
    collect_compiler_diagnostic_sources(sources)
        .iter()
        .map(DsqlDiagnostic::to_transport)
        .collect()
}
