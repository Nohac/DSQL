use dsql_core::{Selection, SourceSnapshot, check_file, parse_source, syntax::Directive};
use std::path::PathBuf;

#[test]
fn directive_shapes_match_expected_ast() {
    let source = r#"
query Users @.deprecated(reason: "Use accounts") {
  users
    @.deprecated(reason: "Use accounts")
    @table.column(label: "Email")
  {
    id @dsql.include_if(if: .id == 1)
  }
}
"#;
    let parsed = parse_source(SourceSnapshot::from(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let mut directives = Vec::new();
    for query in parsed.source_file.queries() {
        let query_name = query
            .name()
            .map_or("<anonymous>", |name| name.text.as_str());
        directives.extend(
            query
                .directives
                .iter()
                .map(|directive| (query_name.to_string(), directive)),
        );
        collect_directives(query_name.to_string(), query.selections(), &mut directives);
    }

    let contents = format!("{directives:#?}");
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!("directives__ast", contents);
    });
}

#[test]
fn directive_validation_reports_builtin_errors() {
    let cases = [
        (
            "valid",
            r#"
query Users @.deprecated(reason: "Use Accounts") {
  users {
    id @.include_if(if: true) @.deprecated
  }
}
"#,
        ),
        (
            "unknown_directive",
            r#"
query Users {
  users @table.column(label: "Email") {
    id
  }
}
"#,
        ),
        (
            "wrong_location",
            r#"
query Users @.include_if(if: true) {
  users {
    id
  }
}
"#,
        ),
        (
            "arguments",
            r#"
query Users {
  users {
    id
      @.include_if
      @.include_if(if: "yes")
      @.deprecated(reason: 1, message: "old", message: "again")
  }
}
"#,
        ),
    ];

    let output = cases
        .iter()
        .map(|(name, source)| {
            let parsed = parse_source(SourceSnapshot::from(*source));
            assert!(
                parsed.diagnostics.is_empty(),
                "{name}: {:?}",
                parsed.diagnostics
            );
            let checked = check_file(&parsed.source_file);
            format!("{name}\n{:#?}", checked.errors)
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!("directives__validation", output);
    });
}

fn collect_directives<'a>(
    parent_path: String,
    selections: impl Iterator<Item = &'a Selection>,
    directives: &mut Vec<(String, &'a Directive)>,
) {
    for selection in selections {
        let selection_path = format!("{parent_path}.{}", selection.name.display_text());
        directives.extend(
            selection
                .directives
                .iter()
                .map(|directive| (selection_path.clone(), directive)),
        );
        collect_directives(selection_path, selection.selections.iter(), directives);
    }
}
