use insta::Settings;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[tokio::test]
async fn generate_project_writes_manifest_and_invokes_typescript_command() {
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(
        project.path(),
        &format!(
            r#"[generate.typescript]
enabled = true
out_dir = "src/generated/dsql"
cmd = ["bun", "{}"]
"#,
            repo_root()
                .join("integrations/typescript/renderers/basic.ts")
                .display()
        ),
    );

    let output = dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();
    assert_eq!(
        output.manifest_path,
        project
            .path()
            .join("dsql/build/manifest.json")
            .to_string_lossy()
    );
    assert_eq!(
        output.operation_paths,
        vec!["operations/MovieInfoLookup.json".to_string()]
    );

    let manifest = fs::read_to_string(project.path().join("dsql/build/manifest.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&manifest).unwrap();
    snapshot("generate_manifest", &manifest);

    let operation = fs::read_to_string(
        project
            .path()
            .join("dsql/build/operations/MovieInfoLookup.json"),
    )
    .unwrap();
    serde_json::from_str::<serde_json::Value>(&operation).unwrap();
    snapshot("generate_operation_movie_info_basic", &operation);

    let generated =
        fs::read_to_string(project.path().join("src/generated/dsql/queries.ts")).unwrap();
    snapshot("generate_typescript_command_output", &generated);

    fs::write(
        project.path().join("src/generated/dsql/usage.ts"),
        format!(
            r#"import {{ dsql, MovieInfoLookupOperation, useQuery }} from "./queries";

const MovieInfoLookup = dsql(`{}`);
MovieInfoLookup satisfies typeof MovieInfoLookupOperation;
useQuery(MovieInfoLookup, {{}});
"#,
            fs::read_to_string(project.path().join("queries/movie-info.dsql")).unwrap()
        ),
    )
    .unwrap();

    let status = Command::new("bun")
        .args([
            "build",
            "src/generated/dsql/usage.ts",
            "--outdir",
            "build/tscheck",
        ])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated TypeScript should compile with bun"
    );
}

#[tokio::test]
async fn generate_project_extracts_queries_from_regex_embedding_resolver() {
    let project = tempfile::tempdir().unwrap();
    create_embedded_project_fixture(project.path());

    let output = dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    assert_eq!(
        output.operation_paths,
        vec!["operations/EmbeddedMovieInfoLookup.json".to_string()]
    );
    let manifest = fs::read_to_string(project.path().join("dsql/build/manifest.json")).unwrap();
    snapshot("generate_embedded_regex_manifest", &manifest);
    let operation = fs::read_to_string(
        project
            .path()
            .join("dsql/build/operations/EmbeddedMovieInfoLookup.json"),
    )
    .unwrap();
    snapshot("generate_embedded_regex_operation", &operation);
}

#[tokio::test]
async fn generate_project_types_embedded_dsql_function_calls() {
    let project = tempfile::tempdir().unwrap();
    create_embedded_project_fixture_with_generate(
        project.path(),
        &format!(
            r#"[generate.typescript]
enabled = true
out_dir = "src/generated/dsql"
cmd = ["bun", "{}"]
"#,
            repo_root()
                .join("integrations/typescript/renderers/basic.ts")
                .display()
        ),
    );

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    fs::write(
        project.path().join("src/usage.ts"),
        r#"import { dsql, EmbeddedMovieInfoLookupOperation, useQuery } from "./generated/dsql/queries";

const MovieInfo = dsql(`
  query EmbeddedMovieInfoLookup {
    movie_info(where .id > 0 order by id asc limit 2) {
      id
      info
    }
  }
`);

MovieInfo satisfies typeof EmbeddedMovieInfoLookupOperation;
const query = useQuery(MovieInfo, {});
query.data?.movie_info[0]?.id satisfies number | undefined;
"#,
    )
    .unwrap();

    let status = Command::new("bun")
        .args(["build", "src/usage.ts", "--outdir", "build/embedded"])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "embedded dsql function calls should infer generated operation types"
    );
}

#[tokio::test]
async fn generate_project_splits_top_level_params_and_contextual_input() {
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(
        project.path(),
        &format!(
            r#"[generate.typescript]
enabled = true
out_dir = "src/generated/dsql"
cmd = ["bun", "{}"]
"#,
            repo_root()
                .join("integrations/typescript/renderers/basic.ts")
                .display()
        ),
    );
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        r#"query MovieInfoLookup {
  movie_info(limit $$) {
    id
    info
    title(limit $title_limit) {
      id
    }
  }
}
"#,
    )
    .unwrap();

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    fs::write(
        project.path().join("src/generated/dsql/usage.ts"),
        format!(
            r#"import {{ dsql, MovieInfoLookupOperation, useQuery }} from "./queries";

const MovieInfoLookup = dsql(`{}`);
MovieInfoLookup satisfies typeof MovieInfoLookupOperation;
useQuery(MovieInfoLookup, {{
  params: {{
    limit: 10,
  }},
  input: {{}},
}});
useQuery(MovieInfoLookup, {{
  input: {{
    movie_info: {{
      body: {{
        title: {{
          clause: {{
            limit: {{
              title_limit: 5,
            }},
          }},
        }},
      }},
    }},
  }},
}});
"#,
            fs::read_to_string(project.path().join("queries/movie-info.dsql")).unwrap()
        ),
    )
    .unwrap();

    let generated =
        fs::read_to_string(project.path().join("src/generated/dsql/queries.ts")).unwrap();
    assert!(
        generated.contains("export type MovieInfoLookupParams = {\n    limit: number;\n};"),
        "expected top-level $$ variables to generate params shape:\n{generated}"
    );
    assert!(
        generated.contains("export type MovieInfoLookupInput = {\n    movie_info: {\n        body: {\n            title: {\n                clause: {\n                    limit: {\n                        title_limit: number;\n                    };\n                };\n            };\n        };\n    };\n};"),
        "expected contextual $ variables to generate input shape:\n{generated}"
    );

    let status = Command::new("bun")
        .args([
            "build",
            "src/generated/dsql/usage.ts",
            "--outdir",
            "build/variables",
        ])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated params/input TypeScript should compile with bun"
    );
}

fn create_project_fixture(root: &Path, generate_config: &str) {
    fs::create_dir_all(root.join("dsql/schema")).unwrap();
    fs::create_dir_all(root.join("queries")).unwrap();

    copy_dir(
        &fixture_root().join("schema/imdb"),
        &root.join("dsql/schema"),
    );
    fs::write(
        root.join("dsql/dsql.toml"),
        format!(
            r#"database_url = "<database url>"
default_schema = "public"
documents = [{{ resolver = "dsql", paths = ["queries"] }}]

{generate_config}"#
        ),
    )
    .unwrap();

    fs::write(
        root.join("queries/movie-info.dsql"),
        r#"query MovieInfoLookup {
  movie_info(where .id > 0 order by id asc limit 2) {
    id
    info
    note
    title(where .episode_nr == 100) {
      title
    }
    info_type {
      info
    }
  }
}
"#,
    )
    .unwrap();
}

fn create_embedded_project_fixture(root: &Path) {
    create_embedded_project_fixture_with_generate(root, "");
}

fn create_embedded_project_fixture_with_generate(root: &Path, generate_config: &str) {
    fs::create_dir_all(root.join("dsql/schema")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();

    copy_dir(
        &fixture_root().join("schema/imdb"),
        &root.join("dsql/schema"),
    );
    fs::write(
        root.join("dsql/dsql.toml"),
        format!(
            r#"database_url = "<database url>"
default_schema = "public"
documents = [{{ resolver = "typescript", paths = ["src/**/*.ts"] }}]

{generate_config}"#
        ),
    )
    .unwrap();

    fs::write(
        root.join("src/movie-info.ts"),
        r#"import { dsql } from "@dsql/typescript";

export const MovieInfo = dsql(`
  query EmbeddedMovieInfoLookup {
    movie_info(where .id > 0 order by id asc limit 2) {
      id
      info
    }
  }
`);
"#,
    )
    .unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    fixture_root().parent().unwrap().to_path_buf()
}

fn snapshot(name: &str, contents: &str) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(fixture_root().join("snapshots"));
    settings.bind(|| {
        insta::assert_snapshot!(name, contents);
    });
}
