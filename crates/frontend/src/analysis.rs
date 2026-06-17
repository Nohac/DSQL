use dsql_core::{
    CompilerDiagnosticSource, Diagnostic, DsqlDiagnostic, collect_compiler_diagnostic_sources,
};

pub(crate) fn collect_diagnostics_parts(
    sources: &[&dyn CompilerDiagnosticSource],
) -> Vec<Diagnostic> {
    collect_compiler_diagnostic_sources(sources)
        .iter()
        .map(DsqlDiagnostic::to_transport)
        .collect()
}
