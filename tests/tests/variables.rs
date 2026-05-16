use dsql_core::{
    Catalog, SourceSnapshot, check_file_with_catalog, infer_variable_bindings, parse_source,
};
use insta::Settings;
use std::path::PathBuf;

#[test]
fn scalar_variables_parse_and_infer_bindings() {
    let source = r#"
query VariableSearch {
  users(where .id > $min_id and .posts.title like $$title limit $ offset $$) {
    id
    posts(limit $post_limit) {
      title
    }
  }
}
"#;
    let catalog = Catalog::hardcoded();
    let parsed = parse_source(SourceSnapshot::from(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let checked = check_file_with_catalog(&parsed.source_file, &catalog);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);

    let bindings = infer_variable_bindings(&parsed.source_file, &catalog);
    let output = bindings
        .bindings
        .iter()
        .map(|binding| {
            format!(
                "{} {:?} {:?} {:?} {:?}",
                binding.path, binding.source, binding.role, binding.data_type, binding.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    snapshot("scalar_variable_bindings", &output);
}

#[test]
fn operator_variables_parse_and_infer_bindings() {
    let source = r#"
query PostSearch {
  posts(where .created_at $[>, >=] $min_created_at and .title $$title_op[==, !=, like] $title limit $) {
    id
  }
}
"#;
    let catalog = Catalog::hardcoded();
    let parsed = parse_source(SourceSnapshot::from(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let checked = check_file_with_catalog(&parsed.source_file, &catalog);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);

    let bindings = infer_variable_bindings(&parsed.source_file, &catalog);
    let output = bindings
        .bindings
        .iter()
        .map(|binding| {
            let operators = binding
                .operators
                .iter()
                .map(|operator| format!("{operator:?}"))
                .collect::<Vec<_>>()
                .join("|");
            format!(
                "{} {:?} {:?} {:?} {:?} [{}]",
                binding.path,
                binding.source,
                binding.role,
                binding.data_type,
                binding.name,
                operators
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    snapshot("operator_variable_bindings", &output);
}

#[test]
fn operator_variables_reject_wildcard_allowlists() {
    let parsed = parse_source(SourceSnapshot::from(
        "query PostSearch { posts(where .title $[*] $title) { id } }",
    ));

    assert!(
        !parsed.diagnostics.is_empty(),
        "$[*] should not parse as an operator allowlist"
    );
}

#[test]
fn order_by_direction_variables_parse_and_infer_bindings() {
    let source = r#"
query PostOrdering {
  posts(order by created_at $created_dir, title $$) {
    id
  }
}
"#;
    let catalog = Catalog::hardcoded();
    let parsed = parse_source(SourceSnapshot::from(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let checked = check_file_with_catalog(&parsed.source_file, &catalog);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);

    let bindings = infer_variable_bindings(&parsed.source_file, &catalog);
    let output = bindings
        .bindings
        .iter()
        .map(|binding| {
            format!(
                "{} {:?} {:?} {:?} {:?} [{}]",
                binding.path,
                binding.source,
                binding.role,
                binding.data_type,
                binding.name,
                binding.enum_values.join("|")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    snapshot("order_by_direction_variable_bindings", &output);
}

fn snapshot(name: &str, contents: &str) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots"));
    settings.bind(|| {
        insta::assert_snapshot!(name, contents);
    });
}
