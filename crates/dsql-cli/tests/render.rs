//! Diagnostic rendering: findings inside embedded regions render under the
//! host file with excerpts in host coordinates.

use bowl::{Bowl, Singleton};
use dsql_cli::render::{collect_diagnostics, render_themed};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::DiagnosticsDemand;
use dsql_core::register_language;
use dsql_core::source::insert_source;
use futures::executor::block_on;
use miette::GraphicalTheme;

#[test]
fn embedded_findings_render_against_the_host_file() {
    block_on(async {
        let bowl = Bowl::new();
        register_language(&bowl).await;
        insert_catalog(&bowl, Catalog::hardcoded()).await;
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;

        let host = "import { dsql } from \"./dsql\";\n\nexport const q = dsql`\nquery Q {\n  users {\n    id\n    nonexistent\n  }\n}\n`;\n";
        insert_source(&bowl, "src/users.ts", host).await;

        let diagnostics = collect_diagnostics(&bowl).await;
        let rendered: Vec<String> = diagnostics
            .iter()
            .map(|diagnostic| render_themed(diagnostic, GraphicalTheme::unicode_nocolor()))
            .collect();
        insta::assert_snapshot!(rendered.join("\n"));
    });
}
