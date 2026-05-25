use insta::Settings;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tokio::sync::{Mutex, MutexGuard};

static GENERATE_TEST_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn generate_project_writes_manifest_and_invokes_typescript_command() {
    let _guard = generate_test_lock().await;
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
                .join("integrations/typescript/renderers/types.ts")
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
    facet_json::from_str::<facet_value::Value>(&manifest).unwrap();
    snapshot("generate_manifest", &manifest);

    let operation = fs::read_to_string(
        project
            .path()
            .join("dsql/build/operations/MovieInfoLookup.json"),
    )
    .unwrap();
    facet_json::from_str::<facet_value::Value>(&operation).unwrap();
    snapshot("generate_operation_movie_info_types", &operation);

    let operations =
        fs::read_to_string(project.path().join("src/generated/dsql/operations.ts")).unwrap();
    snapshot("generate_typescript_operations_output", &operations);
    let dsql = fs::read_to_string(project.path().join("src/generated/dsql/dsql.ts")).unwrap();
    snapshot("generate_typescript_dsql_output", &dsql);
    let index = fs::read_to_string(project.path().join("src/generated/dsql/index.ts")).unwrap();
    assert_eq!(
        index,
        "export * from \"./operations\";\nexport * from \"./dsql\";\n"
    );
    let queries = fs::read_to_string(project.path().join("src/generated/dsql/queries.ts")).unwrap();
    assert_eq!(queries, "export * from \"./index\";\n");

    fs::write(
        project.path().join("src/generated/dsql/usage.ts"),
        format!(
            r#"import {{ dsql, MovieInfoLookupOperation }} from "./queries";

const MovieInfoLookup = dsql(`{}`);
MovieInfoLookup satisfies typeof MovieInfoLookupOperation;
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
async fn generate_project_artifacts_returns_in_memory_metadata() {
    let _guard = generate_test_lock().await;
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(project.path(), "");

    let artifacts = dsql_generate::generate_project_artifacts_from(project.path()).unwrap();

    assert_eq!(artifacts.project_dir, project.path().to_string_lossy());
    assert_eq!(
        artifacts.out_dir,
        project.path().join("src/generated/dsql").to_string_lossy()
    );
    assert_eq!(
        artifacts.manifest_path,
        project
            .path()
            .join("dsql/build/manifest.json")
            .to_string_lossy()
    );
    assert_eq!(artifacts.manifest.operations.len(), 1);
    assert_eq!(artifacts.manifest.fragments.len(), 0);
    assert_eq!(artifacts.operations.len(), 1);
    assert_eq!(artifacts.fragments.len(), 0);
    assert_eq!(artifacts.operations[0].metadata.name, "MovieInfoLookup");
    assert_eq!(
        artifacts.manifest.operations[0].hash,
        artifacts.operations[0].hash
    );
}

#[tokio::test]
async fn generate_project_writes_fragment_artifacts() {
    let _guard = generate_test_lock().await;
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(project.path(), "");
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        r#"fragment MovieCompany on movie_companies {
  note
  company_type {
    kind
  }
}
"#,
    )
    .unwrap();

    let output = dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    assert_eq!(output.operation_paths, Vec::<String>::new());
    let manifest = fs::read_to_string(project.path().join("dsql/build/manifest.json")).unwrap();
    assert!(manifest.contains(r#""fragments": ["#), "{manifest}");
    assert!(manifest.contains(r#""name": "MovieCompany""#), "{manifest}");
    let fragment = fs::read_to_string(
        project
            .path()
            .join("dsql/build/fragments/MovieCompany.json"),
    )
    .unwrap();
    assert!(fragment.contains(r#""kind": "fragment""#), "{fragment}");
    assert!(
        fragment.contains(r#""table": "movie_companies""#),
        "{fragment}"
    );
    assert!(fragment.contains(r#""path": "note""#), "{fragment}");
    assert!(
        fragment.contains(r#""path": "company_type.kind""#),
        "{fragment}"
    );
    assert!(fragment.contains(r#""id": "MovieCompany""#), "{fragment}");
}

#[tokio::test]
async fn generate_project_records_fragment_spread_paths() {
    let _guard = generate_test_lock().await;
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(project.path(), "");
    fs::write(
        project.path().join("queries/fragments.dsql"),
        r#"fragment CompanyTypeInfo on movie_companies {
  company_type {
    kind
  }
}

fragment MovieCompany on movie_companies {
  note
  ...CompanyTypeInfo
}
"#,
    )
    .unwrap();
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        r#"
query CompanyLookup {
  company_name {
    movie_companies {
      company_id
      ...MovieCompany
    }
  }
}
"#,
    )
    .unwrap();

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    let operation = fs::read_to_string(
        project
            .path()
            .join("dsql/build/operations/CompanyLookup.json"),
    )
    .unwrap();
    assert!(
        operation.contains(
            r#""path": "company_name.movie_companies",
      "fragment": "MovieCompany""#
        ),
        "expected direct fragment spread metadata:\n{operation}"
    );
    assert!(
        operation.contains(
            r#""path": "company_name.movie_companies",
      "fragment": "CompanyTypeInfo""#
        ),
        "expected transitive fragment spread metadata:\n{operation}"
    );
}

#[tokio::test]
async fn generate_project_records_fragment_variable_inputs_at_spread_sites() {
    let _guard = generate_test_lock().await;
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(project.path(), "");
    fs::write(
        project.path().join("queries/fragments.dsql"),
        r#"fragment MovieCompany on movie_companies {
  note
  company_type(where .kind like $$kind limit $company_limit) {
    kind
  }
}
"#,
    )
    .unwrap();
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        r#"
query CompanyLookup {
  company_name {
    movie_companies {
      company_id
      ...MovieCompany
    }
  }
}
"#,
    )
    .unwrap();

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    let fragment = fs::read_to_string(
        project
            .path()
            .join("dsql/build/fragments/MovieCompany.json"),
    )
    .unwrap();
    assert!(
        fragment.contains(r#""path": "params.kind""#),
        "fragment $$ variables should be fragment-local params:\n{fragment}"
    );
    assert!(
        fragment.contains(r#""path": "input.company_type.clause.limit.company_limit""#),
        "fragment $ variables should be fragment-local input:\n{fragment}"
    );

    let operation = fs::read_to_string(
        project
            .path()
            .join("dsql/build/operations/CompanyLookup.json"),
    )
    .unwrap();
    assert!(
        operation.contains(
            r#""path": "input.company_name.body.movie_companies.body.MovieCompany.params.kind""#
        ),
        "operation should embed fragment params at the spread site:\n{operation}"
    );
    assert!(
        operation.contains(
            r#""path": "input.company_name.body.movie_companies.body.MovieCompany.input.company_type.clause.limit.company_limit""#
        ),
        "operation should embed fragment input at the spread site:\n{operation}"
    );
}

#[tokio::test]
async fn validate_project_reports_project_diagnostics_without_writing_artifacts() {
    let _guard = generate_test_lock().await;
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(project.path(), "");
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        r#"query BrokenMovieInfo {
  movie_info {
    definitely_not_a_column
  }
}
"#,
    )
    .unwrap();

    let validation = dsql_generate::validate_project_from(project.path()).unwrap();

    assert_eq!(validation.document_count, 1);
    assert_eq!(validation.query_count, 1);
    assert!(
        validation.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("definitely_not_a_column")),
        "expected unknown field diagnostic, got: {:?}",
        validation.diagnostics
    );
    assert!(
        !project.path().join("dsql/build/manifest.json").exists(),
        "validate should not write generated artifacts"
    );
}

#[tokio::test]
async fn generate_project_extracts_queries_from_regex_embedding_resolver() {
    let _guard = generate_test_lock().await;
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
async fn generate_project_supports_user_owned_typescript_entrypoint() {
    let _guard = generate_test_lock().await;
    let project = tempfile::tempdir().unwrap();
    link_typescript_package(project.path());
    create_project_fixture(
        project.path(),
        r#"[generate.typescript]
enabled = true
out_dir = "src/generated/dsql"
cmd = ["bun", "dsql/generate.ts"]
"#,
    );
    write_tanstack_stubs(project.path());
    fs::create_dir_all(project.path().join("dsql/generators")).unwrap();
    fs::create_dir_all(project.path().join("dsql/templates")).unwrap();
    fs::copy(
        repo_root().join("integrations/typescript/renderers/generators/tanstack-query.ts"),
        project.path().join("dsql/generators/tanstack-query.ts"),
    )
    .unwrap();
    fs::copy(
        repo_root().join("integrations/typescript/renderers/generators/tanstack-start.ts"),
        project.path().join("dsql/generators/tanstack-start.ts"),
    )
    .unwrap();
    fs::copy(
        repo_root().join("integrations/typescript/renderers/templates/tanstack-query.ts"),
        project.path().join("dsql/templates/tanstack-query.ts"),
    )
    .unwrap();
    fs::copy(
        repo_root().join("integrations/typescript/renderers/templates/tanstack-start.ts"),
        project.path().join("dsql/templates/tanstack-start.ts"),
    )
    .unwrap();
    fs::write(
        project.path().join("dsql/generate.ts"),
        r#"import {
  loadBuildArtifacts,
  renderDsqlHelper,
  renderTypes,
} from "@dsql/typescript/node";
import { renderTanStackQuery } from "./generators/tanstack-query";
import { renderTanStackStart } from "./generators/tanstack-start";

const manifestPath = process.env.DSQL_MANIFEST;
const outDir = process.env.DSQL_OUT_DIR;

if (!manifestPath || !outDir) {
  throw new Error("DSQL_MANIFEST and DSQL_OUT_DIR are required");
}

const artifacts = loadBuildArtifacts(manifestPath);

await renderTypes(artifacts, { outDir });
await renderDsqlHelper(artifacts, { outDir });
await renderTanStackStart(artifacts, { outDir });
await renderTanStackQuery(artifacts, { outDir });
"#,
    )
    .unwrap();

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    assert!(
        project
            .path()
            .join("src/generated/dsql/operations.ts")
            .exists()
    );
    assert!(project.path().join("src/generated/dsql/dsql.ts").exists());
    assert!(
        project
            .path()
            .join("src/generated/dsql/tanstack-query.ts")
            .exists()
    );
    assert!(
        project
            .path()
            .join("src/generated/dsql/tanstack-start.ts")
            .exists()
    );
    assert!(
        project
            .path()
            .join("src/generated/dsql/tanstack-start.server.ts")
            .exists()
    );
    let operations =
        fs::read_to_string(project.path().join("src/generated/dsql/operations.ts")).unwrap();
    assert!(
        !operations.contains("sql:"),
        "public generated operations should not contain SQL:\n{operations}"
    );
    let tanstack_query =
        fs::read_to_string(project.path().join("src/generated/dsql/tanstack-query.ts")).unwrap();
    assert!(
        !tanstack_query.contains("tanstackQueryTemplate"),
        "generated tanstack-query.ts should be app code, not a template module:\n{tanstack_query}"
    );
    assert!(
        tanstack_query.contains("useTanStackQuery"),
        "generated tanstack-query.ts should wrap TanStack Query:\n{tanstack_query}"
    );
    assert!(
        tanstack_query.contains("executeQuery"),
        "generated tanstack-query.ts should expose direct server execution:\n{tanstack_query}"
    );
    assert!(
        tanstack_query.contains("MovieInfoLookupServerFn"),
        "generated tanstack-query.ts should call generated server functions:\n{tanstack_query}"
    );
    let tanstack_start =
        fs::read_to_string(project.path().join("src/generated/dsql/tanstack-start.ts")).unwrap();
    assert!(
        !tanstack_start.contains("tanstackStartTemplate"),
        "generated tanstack-start.ts should be app code, not a template module:\n{tanstack_start}"
    );
    assert!(
        tanstack_start.contains("createServerFn"),
        "generated tanstack-start.ts should define TanStack Start server functions:\n{tanstack_start}"
    );
    assert!(
        tanstack_start.contains("MovieInfoLookupServerFn"),
        "generated tanstack-start.ts should define an operation-specific server function:\n{tanstack_start}"
    );
    assert!(
        tanstack_start.contains("getGlobalStartContext"),
        "generated tanstack-start.ts should read the executor from TanStack Start request context:\n{tanstack_start}"
    );
    assert!(
        tanstack_start.contains("TanStack Start request context"),
        "generated tanstack-start.ts should explain that the executor comes from request context:\n{tanstack_start}"
    );
    assert!(
        tanstack_start.contains("createServerOnlyFn"),
        "generated tanstack-start.ts should load SQL descriptors through a server-only function:\n{tanstack_start}"
    );
    assert!(
        tanstack_start.contains(r#"import("./tanstack-start.server")"#),
        "generated tanstack-start.ts should load SQL descriptors from the server-only module:\n{tanstack_start}"
    );
    assert!(
        !tanstack_start.contains("configureDsqlServer"),
        "generated tanstack-start.ts should not require a module-level runtime singleton:\n{tanstack_start}"
    );
    assert!(
        !tanstack_start.contains("select\n"),
        "generated tanstack-start.ts should not contain SQL text:\n{tanstack_start}"
    );
    let tanstack_start_server = fs::read_to_string(
        project
            .path()
            .join("src/generated/dsql/tanstack-start.server.ts"),
    )
    .unwrap();
    assert!(
        tanstack_start_server.contains(r#"@tanstack/react-start/server-only"#),
        "SQL descriptor module should be marked server-only:\n{tanstack_start_server}"
    );
    assert!(
        tanstack_start_server.contains("sql:"),
        "server-only SQL descriptor module should contain SQL text:\n{tanstack_start_server}"
    );
    let queries = fs::read_to_string(project.path().join("src/generated/dsql/queries.ts")).unwrap();
    assert!(
        queries.contains(r#"export * from "./tanstack-query";"#),
        "queries.ts should re-export TanStack query helpers for compatibility:\n{queries}"
    );

    fs::write(
        project.path().join("src/entrypoint-usage.ts"),
        r#"import {
  dsql,
  DsqlOperationResult,
  executeQuery,
  MovieInfoLookupOperation,
  queryOptions,
  useQuery,
} from "./generated/dsql/queries";
import type { MovieInfoLookupResult } from "./generated/dsql/queries";
import type { DsqlServerContext } from "./generated/dsql/tanstack-start";
import { serverOperationNames } from "./generated/dsql/tanstack-start";

const context: DsqlServerContext = {
  dsql: {
    executeQuery: async ({ operation, variables, sql, values }) => {
      operation satisfies typeof MovieInfoLookupOperation;
      operation.sql satisfies string;
      sql satisfies string;
      values satisfies any[];
      variables.params satisfies Record<string, never>;
      variables.input satisfies Record<string, never>;
      return {} as never;
    },
  },
};

const MovieInfoLookup = dsql(`query MovieInfoLookup {
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
`);

MovieInfoLookupOperation satisfies typeof MovieInfoLookupOperation;
serverOperationNames satisfies readonly string[];
context.dsql.executeQuery satisfies unknown;
type IsNever<T> = [T] extends [never] ? true : false;
type AssertFalse<T extends false> = T;
type OperationResultIsNotNever = AssertFalse<IsNever<DsqlOperationResult<typeof MovieInfoLookup>>>;
const query = useQuery(MovieInfoLookup);
const options = queryOptions(MovieInfoLookup);
const optionsWithEnabled = queryOptions(MovieInfoLookup, {
  enabled: true,
});
const executed = executeQuery(MovieInfoLookup);
type QueryDataIsNotNever = AssertFalse<IsNever<typeof query.data>>;
type QueryFnData = Awaited<ReturnType<NonNullable<typeof options.queryFn>>>;
type QueryFnDataWithEnabled = Awaited<ReturnType<NonNullable<typeof optionsWithEnabled.queryFn>>>;
type ExecuteData = Awaited<typeof executed>;
type QueryFnDataIsNotNever = AssertFalse<IsNever<QueryFnData>>;
type QueryFnDataWithEnabledIsNotNever = AssertFalse<IsNever<QueryFnDataWithEnabled>>;
type ExecuteDataIsNotNever = AssertFalse<IsNever<ExecuteData>>;
query.data satisfies MovieInfoLookupResult | undefined;
executed satisfies Promise<MovieInfoLookupResult>;
"#,
    )
    .unwrap();
    fs::write(
        project.path().join("src/server.ts"),
        r#"import handler, { createServerEntry } from "@tanstack/react-start/server-entry";
import type { DsqlServerContext } from "./generated/dsql/tanstack-start";
import { MovieInfoLookupOperation } from "./generated/dsql/queries";

type RequestContext = DsqlServerContext;

declare module "@tanstack/react-start" {
  interface Register {
    server: {
      requestContext: RequestContext;
    };
  }
}

const context: RequestContext = {
  dsql: {
    executeQuery: async ({ operation, variables, sql, values }) => {
      operation satisfies typeof MovieInfoLookupOperation;
      operation.sql satisfies string;
      sql satisfies string;
      values satisfies any[];
      variables.params satisfies Record<string, never>;
      variables.input satisfies Record<string, never>;
      return {} as never;
    },
  },
};

export default createServerEntry({
  async fetch(request) {
    return handler.fetch(request, { context });
  },
});
"#,
    )
    .unwrap();

    let status = Command::new("bun")
        .args([
            "build",
            "src/entrypoint-usage.ts",
            "--outdir",
            "build/entrypoint",
        ])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "user-owned TypeScript generation entrypoints should compile generated output"
    );
    let status = Command::new("bun")
        .args(["build", "src/server.ts", "--outdir", "build/server-entry"])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "TanStack Start server entry should compile with DSQL request context"
    );
}

#[tokio::test]
async fn generate_project_types_embedded_dsql_function_calls() {
    let _guard = generate_test_lock().await;
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
                .join("integrations/typescript/renderers/types.ts")
                .display()
        ),
    );

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    fs::write(
        project.path().join("src/usage.ts"),
        r#"import { dsql, EmbeddedMovieInfoLookupOperation } from "./generated/dsql/queries";

const MovieInfo = dsql(`
  query EmbeddedMovieInfoLookup {
    movie_info(where .id > 0 order by id asc limit 2) {
      id
      info
    }
  }
`);

MovieInfo satisfies typeof EmbeddedMovieInfoLookupOperation;
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
async fn generate_project_types_fragment_definitions() {
    let _guard = generate_test_lock().await;
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
                .join("integrations/typescript/renderers/types.ts")
                .display()
        ),
    );
    let fragment_source = r#"fragment MovieCompany on movie_companies {
  note
  company_type(where .kind like $$kind limit $company_limit) {
    kind
  }
}"#;
    let query_source = r#"query CompanyLookup {
  company_name {
    movie_companies {
      company_id
      ...MovieCompany
    }
  }
}

query CompanyLookupFragmentOnly {
  company_name {
    movie_companies {
      ...MovieCompany
    }
  }
}

fragment MovieCompanyLimit on movie_companies {
  company_type(limit $company_limit) {
    kind
  }
}

query CompanyLimitLookup {
  company_name {
    movie_companies {
      ...MovieCompanyLimit
    }
  }
}"#;
    fs::write(
        project.path().join("queries/fragments.dsql"),
        format!("{fragment_source}\n"),
    )
    .unwrap();
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        format!("{query_source}\n"),
    )
    .unwrap();

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    let operations =
        fs::read_to_string(project.path().join("src/generated/dsql/operations.ts")).unwrap();
    assert!(
        operations.contains("export type DsqlFragmentDefinition"),
        "generated operations should export fragment helpers:\n{operations}"
    );
    assert!(
        operations.contains("export type MovieCompanyFragmentResult"),
        "generated operations should export fragment result type:\n{operations}"
    );
    assert!(
        operations.contains("export type MovieCompanyFragmentParams = {")
            && operations.contains("kind: string;"),
        "generated operations should export fragment params type:\n{operations}"
    );
    assert!(
        operations.contains("export type MovieCompanyFragmentInput = {")
            && operations.contains("company_limit: number;"),
        "generated operations should export fragment input type:\n{operations}"
    );
    assert!(
        operations.contains("export const MovieCompanyFragment"),
        "generated operations should export fragment value:\n{operations}"
    );
    assert!(
        operations.contains("movie_companies: Array<MovieCompanyFragmentResult & {"),
        "operation result should compose spread fragment result:\n{operations}"
    );
    assert!(
        operations.contains("movie_companies: Array<MovieCompanyFragmentResult>;"),
        "fragment-only object result should not add an empty object intersection:\n{operations}"
    );
    assert!(
        operations.contains("export type MovieCompanyLimitFragmentVariables = {")
            && operations.contains("params?: MovieCompanyLimitFragmentParams;")
            && operations.contains("input: MovieCompanyLimitFragmentInput;"),
        "generated operations should export fragment variables type:\n{operations}"
    );
    assert!(
        operations.contains("MovieCompanyLimit: MovieCompanyLimitFragmentVariables;"),
        "operation input should reuse fragment variables aliases:\n{operations}"
    );
    assert!(
        operations.contains("export type MovieCompanyFragmentVariables = {")
            && operations.contains("params: MovieCompanyFragmentParams;")
            && operations.contains("input: MovieCompanyFragmentInput;")
            && operations.contains("MovieCompany: MovieCompanyFragmentVariables;"),
        "operation input should reuse required fragment variables aliases when params and input are present:\n{operations}"
    );

    fs::write(
        project.path().join("src/generated/dsql/fragment-usage.ts"),
        format!(
            r#"import {{ dsql, DsqlFragment, DsqlFragmentVariables, MovieCompanyFragment, MovieCompanyLimitFragmentVariables }} from "./queries";

const MovieCompany = dsql(`{fragment_source}`);
MovieCompany satisfies typeof MovieCompanyFragment;

type Props = {{
  item: DsqlFragment<typeof MovieCompany>;
}};

const _props: Props = {{
  item: {{
    note: null,
    company_type: {{ kind: "production companies" }},
  }},
}};

const _vars: DsqlFragmentVariables<typeof MovieCompany> = {{
  params: {{ kind: "%production%" }},
  input: {{
    company_type: {{
      clause: {{ limit: {{ company_limit: 1 }} }},
    }},
  }},
}};

const _limitVars: MovieCompanyLimitFragmentVariables = {{
  input: {{
    company_type: {{
      clause: {{ limit: {{ company_limit: 1 }} }},
    }},
  }},
}};
"#
        ),
    )
    .unwrap();

    let status = Command::new("bun")
        .args([
            "build",
            "src/generated/dsql/fragment-usage.ts",
            "--outdir",
            "build/fragments",
        ])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "generated fragment TypeScript should compile with bun"
    );
}

#[tokio::test]
async fn generate_project_groups_embedded_source_text_for_multiple_queries() {
    let _guard = generate_test_lock().await;
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
                .join("integrations/typescript/renderers/types.ts")
                .display()
        ),
    );
    fs::write(
        project.path().join("src/movie-info.ts"),
        r#"import { dsql } from "@dsql/typescript";

export const MovieInfo = dsql(`
  query FirstEmbeddedMovieInfoLookup {
    movie_info(where .id > 0 order by id asc limit 2) {
      id
      info
    }
  }

  query SecondEmbeddedMovieInfoLookup {
    movie_info(where .id > 0 order by id asc limit 2) {
      id
      note
    }
  }
`);
"#,
    )
    .unwrap();

    dsql_generate::generate_project_from(project.path())
        .await
        .unwrap();

    fs::write(
        project.path().join("src/usage.ts"),
        r#"import {
  dsql,
  FirstEmbeddedMovieInfoLookupOperation,
  SecondEmbeddedMovieInfoLookupOperation,
} from "./generated/dsql/queries";

const MovieInfo = dsql(`
  query FirstEmbeddedMovieInfoLookup {
    movie_info(where .id > 0 order by id asc limit 2) {
      id
      info
    }
  }

  query SecondEmbeddedMovieInfoLookup {
    movie_info(where .id > 0 order by id asc limit 2) {
      id
      note
    }
  }
`);

MovieInfo satisfies
  | typeof FirstEmbeddedMovieInfoLookupOperation
  | typeof SecondEmbeddedMovieInfoLookupOperation;
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
        "embedded dsql documents with multiple queries should compile"
    );
}

#[tokio::test]
async fn generate_project_splits_top_level_params_and_contextual_input() {
    let _guard = generate_test_lock().await;
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
                .join("integrations/typescript/renderers/types.ts")
                .display()
        ),
    );
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        r#"query MovieInfoLookup {
  movie_info(where .id $[==, >] $ limit $$) {
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

    let operation = fs::read_to_string(
        project
            .path()
            .join("dsql/build/operations/MovieInfoLookup.json"),
    )
    .unwrap();
    assert!(
        operation.contains(r#""parameters": ["#),
        "variable operation should emit SQL parameter metadata:\n{operation}"
    );
    assert!(
        operation.contains(r#""path": "params.limit""#),
        "top-level limit variable should become a SQL parameter:\n{operation}"
    );
    assert!(
        operation.contains(r#""path": "input.movie_info.clause.where.id.value""#),
        "anonymous value variable with operator should use the field value path:\n{operation}"
    );
    assert!(
        operation.contains(r#""path": "input.movie_info.clause.where.id.op""#),
        "anonymous operator variable should use the field op path:\n{operation}"
    );
    assert!(
        operation.contains(r#""path": "input.movie_info.body.title.clause.limit.title_limit""#),
        "structured nested limit variable should become a SQL parameter:\n{operation}"
    );
    assert!(
        operation.contains("$1") && operation.contains("$2"),
        "variable SQL should use bind placeholders:\n{operation}"
    );

    fs::write(
        project.path().join("src/generated/dsql/usage.ts"),
        format!(
            r#"import {{ dsql, DsqlOperationResult, MovieInfoLookupInput, MovieInfoLookupOperation, MovieInfoLookupParams }} from "./queries";
import type {{ MovieInfoLookupResult }} from "./queries";

const MovieInfoLookup = dsql(`{}`);
MovieInfoLookup satisfies typeof MovieInfoLookupOperation;
type IsNever<T> = [T] extends [never] ? true : false;
type AssertFalse<T extends false> = T;
type AssertTrue<T extends true> = T;
type OperationResultIsNotNever = AssertFalse<IsNever<DsqlOperationResult<typeof MovieInfoLookup>>>;
type OperationResultMatchesGenerated = AssertTrue<DsqlOperationResult<typeof MovieInfoLookup> extends MovieInfoLookupResult ? true : false>;
const params: MovieInfoLookupParams = {{
  limit: 10,
}};
const input: MovieInfoLookupInput = {{
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
    clause: {{
      where: {{
        id: {{
          op: ">",
          value: 10,
        }},
      }},
    }},
  }},
}};
params satisfies MovieInfoLookupParams;
input satisfies MovieInfoLookupInput;
"#,
            fs::read_to_string(project.path().join("queries/movie-info.dsql")).unwrap()
        ),
    )
    .unwrap();

    let generated =
        fs::read_to_string(project.path().join("src/generated/dsql/operations.ts")).unwrap();
    assert!(
        generated.contains("export type MovieInfoLookupParams = {\n    limit: number;\n};"),
        "expected top-level $$ variables to generate params shape:\n{generated}"
    );
    assert!(generated.contains("id: {\n                    op: \"==\" | \">\";\n                    value: number;\n                };"),
        "expected anonymous value/operator variables to generate a field control object:\n{generated}"
    );
    assert!(
        generated.contains("title_limit: number;"),
        "expected contextual named nested variables to generate input shape:\n{generated}"
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

#[tokio::test]
async fn generate_project_rejects_ambiguous_anonymous_variables() {
    let _guard = generate_test_lock().await;
    let project = tempfile::tempdir().unwrap();
    create_project_fixture(project.path(), "");
    fs::write(
        project.path().join("queries/movie-info.dsql"),
        r#"query MovieInfoLookup {
  movie_info(where .id > $ and .id < $) {
    id
  }
}
"#,
    )
    .unwrap();

    let error = dsql_generate::generate_project_from(project.path())
        .await
        .expect_err("ambiguous anonymous variables should fail generation");
    let message = error.to_string();
    assert!(
        message.contains("multiple anonymous variables")
            && message.contains("input.movie_info.clause.where.id"),
        "expected disambiguation error, got: {message}"
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

async fn generate_test_lock() -> MutexGuard<'static, ()> {
    GENERATE_TEST_LOCK.lock().await
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

fn link_typescript_package(root: &Path) {
    let scope = root.join("node_modules/@dsql");
    fs::create_dir_all(&scope).unwrap();
    symlink_dir(
        &repo_root().join("integrations/typescript"),
        &scope.join("typescript"),
    );
}

fn write_tanstack_stubs(root: &Path) {
    let react_start = root.join("node_modules/@tanstack/react-start");
    fs::create_dir_all(&react_start).unwrap();
    fs::write(
        react_start.join("package.json"),
        r#"{"name":"@tanstack/react-start","type":"module","exports":{".":"./index.ts","./server-entry":"./server-entry.ts","./server-only":"./server-only.ts"}}"#,
    )
    .unwrap();
    fs::write(
        react_start.join("index.ts"),
        r#"export interface Register {}

type ServerFnOptions = {
  readonly method?: string;
};

export function createServerFn(_options?: ServerFnOptions) {
  return {
    inputValidator<Input>(_validator: (input: Input) => Input) {
      return {
        handler<Result>(
          handler: (options: { readonly data: Input }) => Promise<Result> | Result,
        ) {
          return (options: { readonly data: Input }) =>
            Promise.resolve(handler({ data: options.data }));
        },
      };
    },
  };
}

export function getGlobalStartContext(): unknown {
  return undefined;
}

export function createServerOnlyFn<Args extends readonly unknown[], Result>(
  fn: (...args: Args) => Result,
): (...args: Args) => Result {
  return fn;
}
"#,
    )
    .unwrap();
    fs::write(react_start.join("server-only.ts"), "export {};\n").unwrap();
    fs::write(
        react_start.join("server-entry.ts"),
        r#"import type { Register } from "./index";

type RequestContext = Register extends {
  server: { requestContext: infer TRequestContext };
}
  ? TRequestContext
  : undefined;

type RequestOptions = RequestContext extends undefined
  ? { readonly context?: RequestContext }
  : { readonly context: RequestContext };

type ServerEntry = {
  readonly fetch: (request: Request) => Response | Promise<Response>;
};

declare const handler: {
  readonly fetch: (
    request: Request,
    options: RequestOptions,
  ) => Response | Promise<Response>;
};

export function createServerEntry<Entry extends ServerEntry>(entry: Entry): Entry {
  return entry;
}

export default handler;
"#,
    )
    .unwrap();

    let react_query = root.join("node_modules/@tanstack/react-query");
    fs::create_dir_all(&react_query).unwrap();
    fs::write(
        react_query.join("package.json"),
        r#"{"name":"@tanstack/react-query","type":"module","exports":"./index.ts"}"#,
    )
    .unwrap();
    fs::write(
        react_query.join("index.ts"),
        r#"export type UseQueryOptions<
  TQueryFnData = unknown,
  TError = Error,
  TData = TQueryFnData,
  TQueryKey extends readonly unknown[] = readonly unknown[],
> = {
  readonly queryKey: TQueryKey;
  readonly queryFn: () => Promise<TQueryFnData>;
  readonly enabled?: boolean;
  readonly staleTime?: number;
  readonly gcTime?: number;
  readonly retry?: boolean | number;
  readonly select?: (data: TQueryFnData) => TData;
};

export type UseQueryResult<TData = unknown, TError = Error> = {
  readonly data: TData | undefined;
  readonly error: TError | null;
  readonly isError: boolean;
  readonly isLoading: boolean;
  readonly isPending: boolean;
  readonly isSuccess: boolean;
};

export function queryOptions<
  TQueryFnData = unknown,
  TError = Error,
  TData = TQueryFnData,
  TQueryKey extends readonly unknown[] = readonly unknown[],
>(
  options: UseQueryOptions<TQueryFnData, TError, TData, TQueryKey>,
): UseQueryOptions<TQueryFnData, TError, TData, TQueryKey> {
  return options;
}

export function useQuery<
  TQueryFnData = unknown,
  TError = Error,
  TData = TQueryFnData,
  TQueryKey extends readonly unknown[] = readonly unknown[],
>(
  _options: UseQueryOptions<TQueryFnData, TError, TData, TQueryKey>,
): UseQueryResult<TData, TError> {
  return {
    data: undefined,
    error: null,
    isError: false,
    isLoading: false,
    isPending: false,
    isSuccess: true,
  };
}
"#,
    )
    .unwrap();
}

#[cfg(unix)]
fn symlink_dir(from: &Path, to: &Path) {
    std::os::unix::fs::symlink(from, to).unwrap();
}

#[cfg(windows)]
fn symlink_dir(from: &Path, to: &Path) {
    std::os::windows::fs::symlink_dir(from, to).unwrap();
}

fn snapshot(name: &str, contents: &str) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(fixture_root().join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!(format!("generate__{name}"), contents);
    });
}
