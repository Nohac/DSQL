use dsql_core::{
    Catalog, SourceSnapshot, check_file_with_catalog, infer_variable_bindings, parse_source,
};
use insta::Settings;
use std::path::PathBuf;

#[test]
fn variable_bindings_match_expected_shapes() {
    let cases = [
        (
            "scalar",
            r#"
query VariableSearch {
  users(where .id > $min_id and .posts.title like $$title limit $ offset $$) {
    id
    posts(limit $post_limit) {
      title
    }
  }
}
"#,
        ),
        (
            "operator",
            r#"
query PostSearch {
  posts(where .created_at $[>, >=] $min_created_at and .title $$title_op[==, !=, like] $title limit $) {
    id
  }
}
"#,
        ),
        (
            "order_by_direction",
            r#"
query PostOrdering {
  posts(order by created_at $created_dir, title $$) {
    id
  }
}
"#,
        ),
        (
            "fragment",
            r#"
fragment UserPosts on users {
  posts(where .title like $$search limit $post_limit) {
    title
  }
}
"#,
        ),
    ];

    let catalog = Catalog::hardcoded();
    let output = cases
        .iter()
        .map(|(name, source)| {
            let parsed = parse_source(SourceSnapshot::from(*source));
            assert!(
                parsed.diagnostics.is_empty(),
                "{name}: {:?}",
                parsed.diagnostics
            );

            let checked = check_file_with_catalog(&parsed.source_file, &catalog);
            assert!(checked.errors.is_empty(), "{name}: {:?}", checked.errors);

            let bindings = infer_variable_bindings(&parsed.source_file, &catalog);
            let bindings = bindings
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
                        "{} {:?} {:?} {:?} {:?} operators=[{}] enum=[{}]",
                        binding.path,
                        binding.source,
                        binding.role,
                        binding.data_type,
                        binding.name,
                        operators,
                        binding.enum_values.join("|")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("{name}\n{bindings}")
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    snapshot("variable_bindings", &output);
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

fn snapshot(name: &str, contents: &str) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!(format!("variables__{name}"), contents);
    });
}
