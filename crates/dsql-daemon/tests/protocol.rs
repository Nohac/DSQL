//! Build-daemon protocol tests (docs/spec/build-daemon.md): the daemon
//! runs over a duplex pipe and is driven with raw line-JSON, exactly as
//! a build-tool binding drives stdio.

use std::path::{Path, PathBuf};

use facet_value::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

const MOVIE_RATINGS_SCHEMA: &str = r#"---
schema: public
name: movie_ratings
object_type: table
columns:
  - name: id
    provider_type: { schema: pg_catalog, name: int4 }
    database_type: int4
    data_type: int
    not_null: true
  - name: movie_id
    provider_type: { schema: pg_catalog, name: int4 }
    database_type: int4
    data_type: int
    not_null: true
  - name: info_type_id
    provider_type: { schema: pg_catalog, name: int4 }
    database_type: int4
    data_type: int
    not_null: true
  - name: info
    provider_type: { schema: pg_catalog, name: text }
    database_type: text
    data_type: text
    not_null: true
constraints:
  - name: movie_ratings_pkey
    kind: primary_key
    columns:
      - id
foreign_keys:
  - name: movie_ratings_movie_id_fkey
    columns:
      - movie_id
    references:
      schema: public
      table: title
      columns:
        - id
indexes:
  - name: movie_ratings_pkey
    access_method: btree
    keys:
      - column: id
    unique: true
"#;

struct Session {
    input: DuplexStream,
    output: BufReader<DuplexStream>,
    root: PathBuf,
    next_id: u64,
}

impl Session {
    /// Starts the daemon against a temp copy of the scoped fixture,
    /// without initializing.
    async fn start(test: &str) -> Session {
        Self::start_with_lock_mode(test, false).await
    }

    async fn start_locked(test: &str) -> Session {
        Self::start_with_lock_mode(test, true).await
    }

    async fn start_with_lock_mode(test: &str, locked: bool) -> Session {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped");
        let root = std::env::temp_dir().join(format!("dsql-daemon-{test}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clean stale fixture copy");
        }
        copy_tree(&source, &root);

        Self::at_root(root, locked)
    }

    fn at_root(root: PathBuf, locked: bool) -> Session {
        let (client_in, server_in) = tokio::io::duplex(1 << 20);
        let (server_out, client_out) = tokio::io::duplex(1 << 20);
        tokio::spawn(async move {
            dsql_daemon::serve_with_lock_mode(server_in, server_out, locked).await;
        });
        Session {
            input: client_in,
            output: BufReader::new(client_out),
            root,
            next_id: 0,
        }
    }

    /// Starts and initializes.
    async fn ready(test: &str) -> Session {
        let mut session = Session::start(test).await;
        let response = session.initialize().await;
        assert!(
            response.contains("\"result\""),
            "initialize succeeds, got {response}"
        );
        session
    }

    async fn ready_locked(test: &str) -> Session {
        let mut session = Session::start_locked(test).await;
        let response = session.initialize().await;
        assert!(
            response.contains("\"result\""),
            "initialize succeeds, got {response}"
        );
        session
    }

    fn initialize_params(&self) -> String {
        format!(
            "{{\"protocolVersion\":1,\"root\":{}}}",
            json_str(&self.root.to_string_lossy())
        )
    }

    async fn initialize(&mut self) -> String {
        self.request("initialize", &self.initialize_params()).await
    }

    async fn initialize_with_diagnostic_level(&mut self, level: &str) -> String {
        self.request(
            "initialize",
            &format!(
                "{{\"protocolVersion\":1,\"root\":{},\"diagnosticLevel\":{}}}",
                json_str(&self.root.to_string_lossy()),
                json_str(level),
            ),
        )
        .await
    }

    async fn compile(&mut self) -> String {
        self.request("compile", "{}").await
    }

    async fn files_changed(&mut self, path: &str) -> String {
        self.request(
            "filesChanged",
            &format!("{{\"paths\":[{}]}}", json_str(path)),
        )
        .await
    }

    async fn send_line(&mut self, line: &str) {
        self.input.write_all(line.as_bytes()).await.expect("send");
        self.input.write_all(b"\n").await.expect("newline");
    }

    async fn request(&mut self, method: &str, params: &str) -> String {
        self.next_id += 1;
        let id = self.next_id;
        self.send_line(&format!(
            "{{\"id\":{id},\"method\":{},\"params\":{params}}}",
            json_str(method)
        ))
        .await;
        self.read_line().await
    }

    async fn read_line(&mut self) -> String {
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.output.read_line(&mut line),
        )
        .await
        .expect("daemon answers within 30s")
        .expect("daemon alive");
        let line = line.trim().to_string();
        assert_response_envelope(&line);
        line
    }
}

fn assert_response_envelope(line: &str) {
    let response = response_json(line);
    let object = response
        .as_object()
        .expect("daemon response is a JSON object");
    assert!(object.get("id").is_some(), "response has an id: {line}");
    let payloads =
        usize::from(object.get("result").is_some()) + usize::from(object.get("error").is_some());
    assert_eq!(
        payloads, 1,
        "response has exactly one result or error: {line}"
    );
}

fn response_json(line: &str) -> Value {
    facet_json::from_str(line).expect("daemon response is valid JSON")
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .as_object()
        .and_then(|object| object.get(name))
        .expect("response field exists")
}

fn error_data(line: &str) -> Value {
    let response = response_json(line);
    field(field(&response, "error"), "data").clone()
}

fn json_str(value: &str) -> String {
    format!("{value:?}")
}

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("fixture dir");
    for entry in std::fs::read_dir(source).expect("fixture readable") {
        let entry = entry.expect("fixture entry");
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("fixture file copies");
        }
    }
}

/// Handshake edges: requests before initialize, version mismatch (daemon
/// stays uninitialized), double initialize, unknown methods, malformed
/// lines answering with id null, and out-of-range ids.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_and_framing_edges() {
    let mut session = Session::start("lifecycle").await;

    let early = session.compile().await;
    assert!(early.contains("\"NotInitialized\""), "got {early}");
    assert!(error_data(&early).is_null(), "got {early}");

    let mismatch = session
        .request(
            "initialize",
            &format!(
                "{{\"protocolVersion\":99,\"root\":{}}}",
                json_str(&session.root.to_string_lossy())
            ),
        )
        .await;
    assert!(
        mismatch.contains("\"UnsupportedProtocolVersion\"")
            && mismatch.contains("\"daemonVersion\":1"),
        "got {mismatch}"
    );
    assert_eq!(
        field(&error_data(&mismatch), "daemonVersion")
            .as_number()
            .and_then(|number| number.to_u64()),
        Some(1),
        "got {mismatch}",
    );

    // The failed handshake left it uninitialized: a correct one works.
    let ok = session.initialize().await;
    assert!(ok.contains("\"protocolVersion\":1"), "got {ok}");
    assert!(
        ok.contains("\"configPath\":\"dsql/dsql.toml\"")
            && ok.contains("\"overlaysDir\":\"dsql/overlays\"")
            && ok.contains("\"buildDir\":\"dsql/build\""),
        "canonical paths return, got {ok}"
    );

    let double = session.initialize().await;
    assert!(double.contains("\"AlreadyInitialized\""), "got {double}");

    let unknown = session.request("frobnicate", "{}").await;
    assert!(
        unknown.contains(&format!("\"id\":{}", session.next_id))
            && unknown.contains("InvalidRequest")
            && unknown.contains("\"method\":\"frobnicate\""),
        "unknown methods keep their id and name the method, got {unknown}"
    );
    let unknown_data = error_data(&unknown);
    assert_eq!(
        field(&unknown_data, "method")
            .as_string()
            .map(|method| method.as_str()),
        Some("frobnicate"),
        "got {unknown}",
    );

    session.send_line("this is not json").await;
    let malformed = session.read_line().await;
    assert!(
        malformed.contains("\"id\":null") && malformed.contains("InvalidRequest"),
        "got {malformed}"
    );
    assert!(error_data(&malformed).is_null(), "got {malformed}");

    // Invalid UTF-8 answers InvalidRequest without killing the session.
    session
        .input
        .write_all(b"{\"id\":9}\xff\xfe\n")
        .await
        .expect("send bytes");
    let not_utf8 = session.read_line().await;
    assert!(
        not_utf8.contains("\"id\":null") && not_utf8.contains("not UTF-8"),
        "got {not_utf8}"
    );
    assert!(error_data(&not_utf8).is_null(), "got {not_utf8}");

    session
        .send_line("{\"id\":0,\"method\":\"compile\",\"params\":{}}")
        .await;
    let zero = session.read_line().await;
    assert!(
        zero.contains("\"id\":null") && zero.contains("outside"),
        "id 0 rejects, got {zero}"
    );

    let shutdown = session.request("shutdown", "{}").await;
    assert!(shutdown.contains("\"result\":true"), "got {shutdown}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostic_level_filters_success_errors_and_replays() {
    let mut invalid = Session::start("invalid-diagnostic-level").await;
    let rejected = invalid.initialize_with_diagnostic_level("debug").await;
    assert!(
        rejected.contains("\"InvalidRequest\"") && rejected.contains("diagnosticLevel"),
        "invalid levels reject initialization, got {rejected}",
    );
    let recovered = invalid.initialize().await;
    assert!(
        recovered.contains("\"diagnosticLevel\":\"info\""),
        "the default level is info, got {recovered}",
    );
    let mut warnings = Session::start("warning-diagnostic-level").await;
    let warning_level = warnings.initialize_with_diagnostic_level("warning").await;
    assert!(
        warning_level.contains("\"diagnosticLevel\":\"warning\""),
        "warning is an accepted level, got {warning_level}",
    );

    let mut session = Session::start("error-diagnostic-level").await;
    let config = session.root.join("dsql/dsql.toml");
    let mut raw = std::fs::read_to_string(&config).expect("config readable");
    raw.push_str("\n[generate.typescript]\nenabled = true\ncmd = [\"sh\", \"-c\", \"true\"]\n");
    std::fs::write(config, raw).expect("warning-producing config writable");

    let initialized = session.initialize_with_diagnostic_level("error").await;
    assert!(
        initialized.contains("\"diagnosticLevel\":\"error\""),
        "initialize echoes the effective level, got {initialized}",
    );
    let clean = session.compile().await;
    assert!(
        clean.contains("\"result\"")
            && !clean.contains("GeneratorSkipped")
            && clean.contains("\"diagnostics\":[]"),
        "errors-only success snapshots omit warnings, got {clean}",
    );

    std::fs::write(
        session.root.join("queries/frontend/titles.dsql"),
        "query Broken { missing_table { id } }\n",
    )
    .expect("invalid query writable");
    let broken = session.files_changed("queries/frontend/titles.dsql").await;
    assert!(
        broken.contains("\"Diagnostics\"") && broken.contains("TableNotFound"),
        "language errors still block generation, got {broken}",
    );
    assert!(
        !broken.contains("GeneratorSkipped") && !broken.contains("\"severity\":\"Warning\""),
        "errors-only snapshots omit warnings, got {broken}",
    );
    let broken_data = error_data(&broken);
    assert!(
        field(&broken_data, "diagnostics")
            .as_array()
            .is_some_and(|diagnostics| !diagnostics.is_empty()),
        "diagnostics errors carry the snapshot, got {broken}",
    );

    let replay = session.files_changed("README.md").await;
    assert!(
        replay.contains("TableNotFound") && !replay.contains("GeneratorSkipped"),
        "no-op replay preserves the filtered snapshot, got {replay}",
    );
}

/// A full compile answers the spec's result shape: generation id, both
/// manifest paths, artifacts with embedded metadata, group closures,
/// callsites with hashes and full-expression ranges, and a complete
/// diagnostics snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compile_answers_the_result_shape() {
    let mut session = Session::ready("compile").await;
    let response = session.compile().await;

    for expectation in [
        "\"generationId\":1",
        "\"changed\":true",
        "\"manifestPath\":\"dsql/build/manifest.1.json\"",
        "\"currentManifestPath\":\"dsql/build/manifest.json\"",
        "\"projectContractHash\":{\"algorithm\":\"sha256\",\"value\":\"",
        "\"id\":\"frontend/operation/Titles\"",
        "\"id\":\"shared/fragment/TitleBits\"",
        "\"name\":\"frontend\",\"imports\":[\"shared\"],\"generationTarget\":true",
        "\"name\":\"shared\",\"imports\":[],\"generationTarget\":false",
        "\"path\":\"src/components/TitlePanel.ts\"",
        "\"resolver\":\"typescript\"",
        "\"algorithm\":\"sha256\"",
        "\"target\":\"frontend/operation/TitlePanel\"",
        "\"diagnostics\":[]",
    ] {
        assert!(
            response.contains(expectation),
            "missing {expectation} in {response}"
        );
    }
    // The frontend group's closure includes the imported shared fragment.
    let frontend_group = response
        .split(
            "{\"name\":\"frontend\",\"imports\":[\"shared\"],\"generationTarget\":true,\"artifacts\":[",
        )
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("frontend group present");
    assert!(
        frontend_group.contains("shared/fragment/TitleBits"),
        "closure includes imports, got {frontend_group}"
    );
    // The callsite range covers the entire dsql`…` expression.
    let host = std::fs::read_to_string(session.root.join("src/components/TitlePanel.ts"))
        .expect("host readable");
    let range_start = response
        .split("\"expressions\":[{\"range\":{\"start\":")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|start| start.parse::<usize>().ok())
        .expect("expression range present");
    assert_eq!(
        &host[range_start..range_start + 5],
        "dsql`",
        "the range starts at the expression"
    );

    // On disk: content-addressed layout committed via the pointer.
    let manifest = std::fs::read_to_string(session.root.join("dsql/build/manifest.json"))
        .expect("pointer committed");
    assert!(manifest.contains("\"generationId\":1"));
    assert!(session.root.join("dsql/build/manifest.1.json").exists());
    assert!(
        !response.contains("\"definitions\""),
        "callsites expose one opaque target, got {response}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_multiple_roots_target_one_operation_artifact() {
    let mut session = Session::start("multiple-root-callsite").await;
    std::fs::write(
        session.root.join("src/components/Overview.ts"),
        indoc::indoc! {r#"
            export const overview = dsql(`
              query Overview {
                titles: title(limit 2) { id }
                summary: title | aggregate { count }
              }
            `);
        "#},
    )
    .expect("embedded fixture writes");
    let initialized = session.initialize().await;
    assert!(initialized.contains("\"result\""), "got {initialized}");

    let response = session.compile().await;
    for expectation in [
        "\"id\":\"frontend/operation/Overview\"",
        "\"target\":\"frontend/operation/Overview\"",
        "\"name\":\"Overview\"",
    ] {
        assert!(
            response.contains(expectation),
            "missing {expectation} in {response}"
        );
    }
    assert!(
        !response.contains("Overview_titles") && !response.contains("Overview_summary"),
        "roots must not leak synthetic artifact identities: {response}"
    );
}

/// A configured TypeScript extractor owns rewriting independently of the
/// host extension, and fragment-only expressions resolve to fragment
/// artifacts just like queries resolve to operations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn callsites_expose_resolver_and_fragment_target() {
    let mut session = Session::start("fragment-callsite").await;
    let config_path = session.root.join("dsql/dsql.toml");
    let config = std::fs::read_to_string(&config_path).expect("config readable");
    std::fs::write(
        &config_path,
        config.replace(
            "{ resolver = \"custom\", paths = [\"src/**/*.component\"] }",
            "{ resolver = \"typescript\", paths = [\"src/**/*.component\"] }",
        ),
    )
    .expect("config writable");
    std::fs::write(
        session.root.join("src/components/Bits.component"),
        "export const bits = dsql`fragment PanelBits on title { id }`;\n",
    )
    .expect("host writable");
    let initialized = session.initialize().await;
    assert!(initialized.contains("\"result\""), "got {initialized}");

    let response = session.compile().await;
    assert!(
        response.contains("\"path\":\"src/components/Bits.component\"")
            && response.contains("\"resolver\":\"typescript\"")
            && response.contains("\"target\":\"frontend/fragment/PanelBits\""),
        "got {response}"
    );
    assert!(!response.contains("\"definitions\""), "got {response}");
}

/// filesChanged: an edited file recompiles into a new generation, an
/// irrelevant file replays the outcome with changed:false — including a
/// Diagnostics outcome, which must not be wiped by an unrelated event.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_changed_reconciles_and_replays() {
    let mut session = Session::ready("files-changed").await;
    let first = session.compile().await;
    assert!(first.contains("\"generationId\":1"), "got {first}");

    // Irrelevant file: no-op replay, same generation, changed:false.
    std::fs::write(session.root.join("README.md"), "hello").expect("readme");
    let noop = session.files_changed("README.md").await;
    assert!(
        noop.contains("\"generationId\":1") && noop.contains("\"changed\":false"),
        "got {noop}"
    );

    // Real edit: new generation.
    let doc = session.root.join("queries/frontend/titles.dsql");
    let text = std::fs::read_to_string(&doc).expect("doc readable");
    std::fs::write(&doc, text.replace("limit 2", "limit 5")).expect("edit");
    let second = session.files_changed("queries/frontend/titles.dsql").await;
    assert!(
        second.contains("\"generationId\":2") && second.contains("\"changed\":true"),
        "got {second}"
    );

    // Break the file: Diagnostics error, build tree untouched.
    let text = std::fs::read_to_string(&doc).expect("doc readable");
    std::fs::write(&doc, text.replace("...TitleBits", "bogus_column")).expect("break");
    let broken = session.files_changed("queries/frontend/titles.dsql").await;
    assert!(
        broken.contains("\"Diagnostics\"") && broken.contains("bogus_column"),
        "got {broken}"
    );
    let pointer = std::fs::read_to_string(session.root.join("dsql/build/manifest.json"))
        .expect("pointer intact");
    assert!(
        pointer.contains("\"generationId\":2"),
        "a failed compile leaves the last build tree current"
    );

    // Unrelated event now: the Diagnostics outcome replays — a clean
    // earlier result must not resurface.
    std::fs::write(session.root.join("README.md"), "hello again").expect("readme");
    let replay = session.files_changed("README.md").await;
    assert!(
        replay.contains("\"Diagnostics\"") && replay.contains("bogus_column"),
        "the error outcome replays, got {replay}"
    );

    // Repairing incrementally works without a restart.
    let text = std::fs::read_to_string(&doc).expect("doc readable");
    std::fs::write(&doc, text.replace("bogus_column", "...TitleBits")).expect("repair");
    let repaired = session.files_changed("queries/frontend/titles.dsql").await;
    assert!(
        repaired.contains("\"result\"") && repaired.contains("\"changed\":"),
        "got {repaired}"
    );

    // Outside paths reject.
    let outside = session.files_changed("/etc/passwd").await;
    assert!(outside.contains("\"InvalidPath\""), "got {outside}");
    let outside_data = error_data(&outside);
    assert_eq!(
        field(&outside_data, "path")
            .as_string()
            .map(|path| path.as_str()),
        Some("/etc/passwd"),
        "got {outside}",
    );
}

/// Policy definitions are ambient semantic inputs for consuming queries: an
/// incremental policy-only edit must republish the affected artifacts just as
/// a cold compile would.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_body_edits_republish_dependent_artifacts() {
    let mut session = Session::start("policy-body-edit").await;
    std::fs::write(
        session.root.join("queries/shared/fragments.dsql"),
        "fragment TitleBits on title { id filtered_kind: kind_type { id } }\n",
    )
    .expect("nested fragment writable");
    let policy = session.root.join("queries/shared/policies.dsql");
    std::fs::write(
        &policy,
        "filter KindFilter on kind_type { apply where true where .id > 900001 }\n",
    )
    .expect("policy writable");
    let initialized = session.initialize().await;
    assert!(initialized.contains("\"result\""), "got {initialized}");

    let first = session.compile().await;
    assert!(
        first.contains("\"generationId\":1") && first.contains("900001"),
        "initial policy reaches the artifact, got {first}",
    );

    std::fs::write(
        &policy,
        "filter KindFilter on kind_type { apply where true where .id > 900002 }\n",
    )
    .expect("policy edit writable");
    let changed = session.files_changed("queries/shared/policies.dsql").await;
    assert!(
        changed.contains("\"generationId\":2")
            && changed.contains("\"changed\":true")
            && changed.contains("900002")
            && !changed.contains("900001"),
        "policy edit must publish its new semantics, got {changed}",
    );
}

/// Resolver-bearing document configuration drives cold discovery and warm
/// reconciliation: unmatched files are ignored while a new custom-extension
/// host joins its scope with the configured extractor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_extraction_keeps_warm_and_cold_discovery_aligned() {
    let mut session = Session::start("source-classification").await;
    let config = session.root.join("dsql/dsql.toml");
    let raw = std::fs::read_to_string(&config).expect("config readable");
    let raw = raw.replace(
        "{ resolver = \"dsql\", paths = [\"queries/frontend/**/*.dsql\"] },",
        "{ resolver = \"dsql\", paths = [\"queries/frontend/**/*.dsql\", \"single/*.dsql\"] },",
    );
    std::fs::write(&config, raw).expect("config with single-star assignment");
    std::fs::create_dir_all(session.root.join("broad")).expect("broad dir");
    std::fs::write(session.root.join("broad/cold.md"), "not dsql\n").expect("cold foreign");

    let initialized = session.initialize().await;
    assert!(initialized.contains("\"result\""), "got {initialized}");
    let first = session.compile().await;
    assert!(first.contains("\"generationId\":1"), "got {first}");

    std::fs::write(session.root.join("broad/warm.md"), "not dsql either\n").expect("warm foreign");
    let foreign = session.files_changed("broad/warm.md").await;
    assert!(
        foreign.contains("\"generationId\":1") && foreign.contains("\"changed\":false"),
        "an unmatched foreign file is a no-op, got {foreign}"
    );

    std::fs::create_dir_all(session.root.join("single/nested")).expect("single-star dirs");
    std::fs::write(
        session.root.join("single/direct.dsql"),
        "query FlatWarm { kind_type { kind } }\n",
    )
    .expect("direct single-star match");
    let direct = session.files_changed("single/direct.dsql").await;
    assert!(
        direct.contains("\"id\":\"frontend/operation/FlatWarm\"")
            && direct.contains("\"generationId\":2")
            && direct.contains("\"changed\":true"),
        "a direct child matches a single-star assignment warm, got {direct}"
    );

    std::fs::write(
        session.root.join("single/nested/deep.dsql"),
        "query NestedIgnored { kind_type { kind } }\n",
    )
    .expect("nested single-star non-match");
    let nested = session.files_changed("single/nested/deep.dsql").await;
    assert!(
        nested.contains("\"generationId\":2") && nested.contains("\"changed\":false"),
        "a single star does not cross a separator warm, got {nested}"
    );

    std::fs::write(
        session.root.join("src/Fresh.component"),
        "export const fresh = dsql`query Fresh { kind_type { kind } }`;\n",
    )
    .expect("fresh host");
    let host = session.files_changed("src/Fresh.component").await;
    assert!(
        host.contains("\"id\":\"frontend/operation/Fresh\"") && host.contains("\"changed\":true"),
        "a configured host joins the scope warm regardless of extension, got {host}"
    );
}

/// EOF exits the daemon (bounded); shutdown answers first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eof_exits() {
    let mut session = Session::start("eof").await;
    drop(session.input);
    // The daemon should close its output promptly.
    let mut line = String::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        session.output.read_line(&mut line),
    )
    .await
    .expect("daemon exits on EOF");
    assert_eq!(read.expect("clean close"), 0, "output closes without data");
}

/// The daemon-side generator policy: enabled commands with declared
/// outputs run with the per-generation DSQL_MANIFEST; without declared
/// outputs they are skipped with a warning diagnostic. Reserved roots
/// (generator outputs) never load as project input, even when they
/// contain .dsql files.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generator_policy_and_reserved_roots() {
    let mut session = Session::start("generator").await;
    // Enable a generator that records its env, with declared outputs
    // INSIDE a broad scope glob (src/generated under frontend's
    // src/**/*.ts — the deliberate reserved-inside-scope case); plant a
    // decoy host the scope would otherwise ingest.
    let config = session.root.join("dsql/dsql.toml");
    let mut raw = std::fs::read_to_string(&config).expect("config readable");
    raw.push_str(
        "\n[generate.typescript]\nenabled = true\ncmd = [\"sh\", \"-c\", \"printf %s \\\"$DSQL_MANIFEST\\\" > generator-manifest.txt\"]\noutputs = [\"src/generated\"]\n",
    );
    std::fs::write(&config, raw).expect("config with generator");
    std::fs::create_dir_all(session.root.join("src/generated")).expect("output dir");
    std::fs::write(
        session.root.join("src/generated/decoy.ts"),
        "export const decoy = dsql`\nquery Decoy {\n  bogus {\n    id\n  }\n}\n`;\n",
    )
    .expect("decoy");

    let init = session.initialize().await;
    assert!(
        init.contains("\"generatorOutputs\":[\"src/generated\"]"),
        "outputs return from initialize, got {init}"
    );

    let compiled = session.compile().await;
    assert!(
        compiled.contains("\"result\"") && !compiled.contains("Decoy"),
        "reserved roots never load as input, got {compiled}"
    );
    let manifest_env = std::fs::read_to_string(session.root.join("generator-manifest.txt"))
        .expect("generator ran");
    assert!(
        manifest_env.contains("manifest.1.json"),
        "DSQL_MANIFEST names the immutable manifest, got {manifest_env}"
    );

    // Drop the outputs declaration: the daemon skips the generator and
    // says so with a warning diagnostic.
    let raw = std::fs::read_to_string(&config).expect("config readable");
    std::fs::write(&config, raw.replace("outputs = [\"src/generated\"]\n", ""))
        .expect("config without outputs");
    std::fs::remove_file(session.root.join("generator-manifest.txt")).expect("reset");
    let recompiled = session.files_changed("dsql/dsql.toml").await;
    assert!(
        recompiled.contains("GeneratorSkipped"),
        "the skip warning reports, got {recompiled}"
    );
    assert!(
        !session.root.join("generator-manifest.txt").exists(),
        "the undeclared generator must not run"
    );
}

/// Config reload failure answers ProjectLoadFailed and repairs on the
/// next change; excludeRoots validate at initialize.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_reload_and_exclude_root_validation() {
    let mut session = Session::start("reload").await;
    let bad = session
        .request(
            "initialize",
            &format!(
                "{{\"protocolVersion\":1,\"root\":{},\"excludeRoots\":[\"/abs\"]}}",
                json_str(&session.root.to_string_lossy())
            ),
        )
        .await;
    assert!(bad.contains("\"InvalidPath\""), "got {bad}");

    let ok = session
        .request(
            "initialize",
            &format!(
                "{{\"protocolVersion\":1,\"root\":{},\"excludeRoots\":[\"src/generated\"]}}",
                json_str(&session.root.to_string_lossy())
            ),
        )
        .await;
    assert!(ok.contains("\"result\""), "got {ok}");
    session.compile().await;

    // Break the config: full reload fails, ProjectLoadFailed.
    let config = session.root.join("dsql/dsql.toml");
    let good = std::fs::read_to_string(&config).expect("config readable");
    std::fs::write(&config, "this is [not toml").expect("broken config");
    let failed = session.files_changed("dsql/dsql.toml").await;
    assert!(failed.contains("\"ProjectLoadFailed\""), "got {failed}");

    // Repairing the config recovers without a restart.
    std::fs::write(&config, good).expect("config restored");
    let recovered = session.files_changed("dsql/dsql.toml").await;
    assert!(recovered.contains("\"result\""), "got {recovered}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlay_changes_reload_fail_closed_and_recover() {
    let mut session = Session::ready("overlay-reload").await;
    session.compile().await;
    let overlay = session.root.join("dsql/overlays/catalog.yaml");
    std::fs::create_dir_all(overlay.parent().expect("overlay parent")).expect("overlay directory");
    std::fs::write(
        &overlay,
        "version: 1\nobjects:\n  - target:\n      schema: public\n      name: title\n    hidden: true\n",
    )
    .expect("valid semantic overlay");
    let changed = session.files_changed("dsql/overlays/catalog.yaml").await;
    assert!(
        changed.contains("\"TableNotFound\"") && !changed.contains("\"ProjectLoadFailed\""),
        "a valid overlay edit reloads the effective catalog, got {changed}"
    );

    std::fs::write(
        &overlay,
        "version: 1\nobjects:\n  - target:\n      schema: public\n      name: title\n    hidden: false\n",
    )
    .expect("invalid overlay");

    let failed = session.files_changed("dsql/overlays/catalog.yaml").await;
    assert!(
        failed.contains("\"ProjectLoadFailed\"")
            && failed.contains("\"path\"")
            && failed.contains("catalog.yaml"),
        "overlay reload reports its source path, got {failed}"
    );

    std::fs::write(&overlay, "version: 1\nobjects: []\n").expect("repair overlay");
    let recovered = session.files_changed("dsql/overlays/catalog.yaml").await;
    assert!(
        recovered.contains("\"result\"") && !recovered.contains("\"TableNotFound\""),
        "overlay repair reloads without restarting, got {recovered}"
    );
}

/// Same-content disk events and irrelevant deletions are protocol
/// no-ops; deleting a real project document reloads and drops its
/// artifact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_precision() {
    let mut session = Session::ready("precision").await;
    let first = session.compile().await;
    assert!(first.contains("\"generationId\":1"), "got {first}");

    // Touch a relevant file without changing content: no-op replay.
    let doc = session.root.join("queries/frontend/titles.dsql");
    let text = std::fs::read_to_string(&doc).expect("doc readable");
    std::fs::write(&doc, &text).expect("touch");
    let touched = session.files_changed("queries/frontend/titles.dsql").await;
    assert!(
        touched.contains("\"generationId\":1") && touched.contains("\"changed\":false"),
        "same-content events are no-ops, got {touched}"
    );

    // Deleting an irrelevant file: still a no-op.
    std::fs::write(session.root.join("notes.txt"), "x").expect("notes");
    std::fs::remove_file(session.root.join("notes.txt")).expect("remove notes");
    let deleted_irrelevant = session.files_changed("notes.txt").await;
    assert!(
        deleted_irrelevant.contains("\"changed\":false"),
        "irrelevant deletions are no-ops, got {deleted_irrelevant}"
    );

    // Deleting a real document reloads; its artifact leaves the manifest.
    std::fs::remove_file(&doc).expect("remove doc");
    let deleted = session.files_changed("queries/frontend/titles.dsql").await;
    assert!(
        deleted.contains("\"changed\":true")
            && !deleted.contains("\"id\":\"frontend/operation/Titles\""),
        "relevant deletions reload and drop the artifact, got {deleted}"
    );
}

/// A plain document edited away and back (A -> B -> A) must compile at
/// every step: restoring earlier content revisits an already-seen text
/// fingerprint, which must not leave stale facts from the intermediate
/// version behind (imdsql repro: the restore answered FieldNotFound on
/// every relation column with spans from the intermediate text; the
/// engine heals ambient consumers of the revisited state at settle
/// convergence — porridge bdebf49 + 8670456 — while e81194e completes
/// derived-pair reconciliation, so both edits apply incrementally).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_document_edits_roundtrip() {
    let mut session = Session::start("aba-roundtrip").await;
    // The reproducing shape (bisected from the imdsql project): a
    // definition carrying a clause on a reverse to-many relation. The
    // fixture schema has no to-many into `title`, so add one.
    std::fs::write(
        session.root.join("dsql/schema/public/movie_ratings.yaml"),
        MOVIE_RATINGS_SCHEMA,
    )
    .expect("schema write");
    let doc = session.root.join("queries/shared/fragments.dsql");
    let original = "fragment TitleBits on title {\n  id\n  title\n}\n\n\
                    fragment RatingBits on title {\n  ratings: movie_ratings(where .info_type_id == 101 order by id asc limit 1) {\n    info\n  }\n}\n";
    std::fs::write(&doc, original).expect("fragments write");
    let init = session.initialize().await;
    assert!(init.contains("\"result\""), "got {init}");
    let first = session.compile().await;
    assert!(first.contains("\"generationId\":1"), "got {first}");

    let probed = original.replace(
        "fragment TitleBits on title {\n  id\n",
        "fragment TitleBits on title {\n  id\n  probe_year: production_year\n",
    );
    assert_ne!(probed, original, "the probe must apply");

    std::fs::write(&doc, &probed).expect("probe writes");
    let edited = session.files_changed("queries/shared/fragments.dsql").await;
    assert!(
        edited.contains("\"generationId\":2") && edited.contains("probe_year"),
        "the probe edit compiles, got {edited}"
    );

    // Restore: the SAME bytes the first generation compiled, applied
    // incrementally. Revisiting an already-seen text fingerprint must
    // not strand span-keyed facts from the intermediate version.
    std::fs::write(&doc, original).expect("restore writes");
    let restored = session.files_changed("queries/shared/fragments.dsql").await;
    assert!(
        restored.contains("\"generationId\":3") && !restored.contains("probe_year"),
        "restoring earlier content compiles cleanly, got {restored}"
    );

    // The same roundtrip driven by DIRECTORY events: subtree
    // reconciliation must apply the identical revisit rule.
    std::fs::write(&doc, &probed).expect("probe writes again");
    let edited = session.files_changed("queries/shared").await;
    assert!(
        edited.contains("\"generationId\":4") && edited.contains("probe_year"),
        "the directory-event probe compiles, got {edited}"
    );
    std::fs::write(&doc, original).expect("restore writes again");
    let restored = session.files_changed("queries/shared").await;
    assert!(
        restored.contains("\"generationId\":5") && !restored.contains("probe_year"),
        "restoring earlier content through a directory event compiles cleanly, got {restored}"
    );
}

/// The host-file variant of the A -> B -> A roundtrip: the revisited
/// content lives in an extracted region, and the clause-bearing
/// selection comes from a fragment in another file. Porridge e81194e
/// reconciles the removed pair-bound invocations, so restoration applies
/// incrementally without a daemon reload-on-revisit workaround. The
/// dsql-core host regression pins the same engine guarantee.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_document_edits_roundtrip() {
    let mut session = Session::start("host-aba-roundtrip").await;
    std::fs::write(
        session.root.join("dsql/schema/public/movie_ratings.yaml"),
        MOVIE_RATINGS_SCHEMA,
    )
    .expect("schema write");
    std::fs::write(
        session.root.join("queries/shared/fragments.dsql"),
        "fragment TitleBits on title {\n  id\n  title\n}\n\nfragment RatingBits on title {\n  ratings: movie_ratings(where .info_type_id == 101 order by id asc limit 1) {\n    info\n  }\n}\n",
    )
    .expect("fragments write");
    let host = session.root.join("src/components/Consumer.ts");
    let original = "export const ConsumerQuery = dsql(`\nquery Consumer {\n  title(limit 2) {\n    id\n    ...RatingBits\n  }\n}\n`);\n";
    std::fs::write(&host, original).expect("host write");
    let init = session.initialize().await;
    assert!(init.contains("\"result\""), "got {init}");
    let first = session.compile().await;
    assert!(first.contains("\"generationId\":1"), "got {first}");

    let probed = original.replace(
        "  title(limit 2) {\n    id\n",
        "  title(limit 2) {\n    id\n    probe_year: production_year\n",
    );
    assert_ne!(probed, original, "the probe must apply");
    std::fs::write(&host, &probed).expect("probe writes");
    let edited = session.files_changed("src/components/Consumer.ts").await;
    assert!(
        edited.contains("\"generationId\":2") && edited.contains("probe_year"),
        "the probe edit compiles, got {edited}"
    );

    std::fs::write(&host, original).expect("restore writes");
    let restored = session.files_changed("src/components/Consumer.ts").await;
    assert!(
        restored.contains("\"generationId\":3") && !restored.contains("probe_year"),
        "restoring earlier host content compiles cleanly, got {restored}"
    );
}

/// A failing host generator answers GeneratorFailed naming the committed
/// generation, and the failure replays on unrelated events.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generator_failure_reports_and_replays() {
    let mut session = Session::start("generator-fail").await;
    let config = session.root.join("dsql/dsql.toml");
    let mut raw = std::fs::read_to_string(&config).expect("config readable");
    raw.push_str(
        "\n[generate.typescript]\nenabled = true\ncmd = [\"sh\", \"-c\", \"exit 3\"]\noutputs = [\"src/generated\"]\n",
    );
    std::fs::write(&config, raw).expect("config with failing generator");

    let init = session.initialize().await;
    assert!(init.contains("\"result\""), "got {init}");

    let failed = session.compile().await;
    assert!(
        failed.contains("\"GeneratorFailed\"")
            && failed.contains("\"generationId\":1")
            && failed.contains("manifest.1.json"),
        "the committed generation is named, got {failed}"
    );
    let failed_data = error_data(&failed);
    assert_eq!(
        field(&failed_data, "generationId")
            .as_number()
            .and_then(|number| number.to_u64()),
        Some(1),
        "got {failed}",
    );
    assert!(
        field(&failed_data, "manifestPath")
            .as_string()
            .is_some_and(|path| path.as_str().ends_with("manifest.1.json")),
        "got {failed}",
    );
    assert!(
        session.root.join("dsql/build/manifest.json").exists(),
        "the build tree committed before the generator ran"
    );

    // Unrelated event: the failure replays, the generator is not retried.
    std::fs::write(session.root.join("README.md"), "x").expect("readme");
    let replay = session.files_changed("README.md").await;
    assert!(
        replay.contains("\"GeneratorFailed\""),
        "the failure outcome replays, got {replay}"
    );
}

/// A held publication lock answers PublicationLocked without writing;
/// releasing it lets the next compile publish.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_contention_surfaces() {
    let mut session = Session::ready("contention").await;
    let build_dir = session.root.join("dsql/build");
    std::fs::create_dir_all(&build_dir).expect("build dir");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(build_dir.join(".lock"))
        .expect("lock file");
    let mut lock = fd_lock::RwLock::new(lock_file);
    let guard = lock.write().expect("held by the test");

    let locked = session.compile().await;
    assert!(locked.contains("\"PublicationLocked\""), "got {locked}");
    assert!(error_data(&locked).is_null(), "got {locked}");
    assert!(
        !build_dir.join("manifest.json").exists(),
        "a locked-out compile writes nothing"
    );

    drop(guard);
    let unlocked = session.compile().await;
    assert!(unlocked.contains("\"generationId\":1"), "got {unlocked}");
}

fn add_filter(session: &Session) {
    std::fs::write(
        session.root.join("queries/shared/access.dsql"),
        "filter VisibleTitles on title { where .id > 0 }\n",
    )
    .expect("filter document written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unlocked_daemon_maintains_match_lock_and_ignores_its_own_write() {
    let mut session = Session::start("match-lock-unlocked").await;
    add_filter(&session);
    let initialized = session.initialize().await;
    assert!(initialized.contains("\"result\""), "got {initialized}");

    let first = session.compile().await;
    assert!(first.contains("\"generationId\":1"), "got {first}");
    let lock_path = session.root.join("dsql/dsql.lock");
    let canonical = std::fs::read_to_string(&lock_path).expect("compile writes dsql.lock");
    assert!(canonical.contains("VisibleTitles"));

    let own_event = session.files_changed("dsql/dsql.lock").await;
    assert!(
        own_event.contains("\"generationId\":1") && own_event.contains("\"changed\":false"),
        "the daemon's byte-identical write is a no-op replay: {own_event}",
    );
    assert!(!session.root.join("dsql/build/manifest.2.json").exists());

    std::fs::write(&lock_path, "version: 1\nfilters: []\n").expect("stale lock written");
    let repaired = session.files_changed("dsql/dsql.lock").await;
    assert!(repaired.contains("\"generationId\":2"), "got {repaired}");
    assert_eq!(
        std::fs::read_to_string(&lock_path).expect("repaired lock readable"),
        canonical,
    );

    std::fs::remove_file(&lock_path).expect("lock removed externally");
    let restored = session.files_changed("dsql/dsql.lock").await;
    assert!(restored.contains("\"generationId\":3"), "got {restored}");
    assert_eq!(
        std::fs::read_to_string(&lock_path).expect("restored lock readable"),
        canonical,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locked_daemon_requires_match_and_initializes_watcher_hash() {
    let mut missing = Session::ready_locked("match-lock-missing").await;
    add_filter(&missing);
    let missing_response = missing.compile().await;
    assert!(
        missing_response.contains("\"Diagnostics\"")
            && missing_response.contains("FilterMatchLock")
            && missing_response.contains("dsql/dsql.lock"),
        "missing locked state is a diagnostic: {missing_response}",
    );
    assert!(!missing.root.join("dsql/build/manifest.json").exists());

    let mut unlocked = Session::start("match-lock-restart").await;
    add_filter(&unlocked);
    let initialized = unlocked.initialize().await;
    assert!(initialized.contains("\"result\""), "got {initialized}");
    let first = unlocked.compile().await;
    assert!(first.contains("\"generationId\":1"), "got {first}");
    let root = unlocked.root.clone();
    let lock_path = root.join("dsql/dsql.lock");
    let accepted = std::fs::read_to_string(&lock_path).expect("accepted lock readable");
    drop(unlocked);

    let mut locked = Session::at_root(root.clone(), true);
    let initialized = locked.initialize().await;
    assert!(initialized.contains("\"result\""), "got {initialized}");
    let compiled = locked.compile().await;
    assert!(compiled.contains("\"generationId\":2"), "got {compiled}");

    let restart_event = locked.files_changed("dsql/dsql.lock").await;
    assert!(
        restart_event.contains("\"generationId\":2") && restart_event.contains("\"changed\":false"),
        "initialization hashes the accepted lock for watcher replay: {restart_event}",
    );
    assert!(!root.join("dsql/build/manifest.3.json").exists());

    std::fs::write(&lock_path, "version: 1\nfilters: []\n").expect("stale lock written");
    let stale = locked.files_changed("dsql/dsql.lock").await;
    assert!(
        stale.contains("\"Diagnostics\"") && stale.contains("FilterMatchLock"),
        "stale locked state is a diagnostic: {stale}",
    );
    assert!(!root.join("dsql/build/manifest.3.json").exists());

    std::fs::write(&lock_path, accepted).expect("accepted lock restored");
    let recovered = locked.files_changed("dsql/dsql.lock").await;
    assert!(recovered.contains("\"generationId\":3"), "got {recovered}");

    std::fs::remove_file(&lock_path).expect("accepted lock deleted");
    let deleted = locked.files_changed("dsql/dsql.lock").await;
    assert!(
        deleted.contains("\"Diagnostics\"") && deleted.contains("FilterMatchLock"),
        "deleted locked state is a diagnostic: {deleted}",
    );
    assert!(!root.join("dsql/build/manifest.4.json").exists());
}

/// EOF while a compile is in flight: the daemon finishes the transaction
/// (the build tree commits), suppresses the response, and exits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eof_mid_flight_finishes_the_transaction() {
    let mut session = Session::ready("eof-flight").await;
    // Fire a compile and slam the input shut before it can answer.
    session
        .send_line("{\"id\":2,\"method\":\"compile\",\"params\":{}}")
        .await;
    drop(session.input);

    // The output must close without ever carrying the compile response.
    let mut collected = String::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        session.output.read_line(&mut collected),
    )
    .await
    .expect("daemon exits after finishing in-flight work");
    assert_eq!(
        read.expect("clean close"),
        0,
        "no response after EOF, got {collected}"
    );
    assert!(
        session.root.join("dsql/build/manifest.json").exists(),
        "the in-flight publication still committed"
    );
}

/// New files join their scope warm; newly ambiguous files behave exactly
/// like a cold reload (the loader's ownership error); an unreadable
/// reserved subtree never affects discovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_files_ambiguity_and_unreadable_reserved_roots() {
    let mut session = Session::start("new-files").await;
    // Declare a reserved output root and make it unreadable: discovery
    // must never traverse it.
    let config = session.root.join("dsql/dsql.toml");
    let mut raw = std::fs::read_to_string(&config).expect("config readable");
    raw.push_str("\n[generate.typescript]\noutputs = [\"src/generated\"]\n");
    std::fs::write(&config, raw).expect("config with outputs");
    let sealed = session.root.join("src/generated");
    std::fs::create_dir_all(&sealed).expect("output dir");
    std::fs::write(sealed.join("decoy.ts"), "export const x = 1;\n").expect("decoy");
    let mut permissions = std::fs::metadata(&sealed).expect("meta").permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o000);
    std::fs::set_permissions(&sealed, permissions).expect("seal");

    let init = session.initialize().await;
    assert!(init.contains("\"result\""), "got {init}");
    let compiled = session.compile().await;
    assert!(
        compiled.contains("\"generationId\":1"),
        "an unreadable reserved subtree must not break discovery, got {compiled}"
    );

    // A brand-new scoped file joins warm and its artifact appears.
    std::fs::write(
        session.root.join("queries/frontend/fresh.dsql"),
        "query Fresh {\n  kind_type {\n    kind\n  }\n}\n",
    )
    .expect("fresh doc");
    let fresh = session.files_changed("queries/frontend/fresh.dsql").await;
    assert!(
        fresh.contains("\"id\":\"frontend/operation/Fresh\"") && fresh.contains("\"changed\":true"),
        "new files join their scope warm, got {fresh}"
    );

    // Two resolvers in one scope overlap from here on (no overlapping FILES
    // yet, so the reload stays clean); creating a file both assignments match
    // exercises warm Relevance::Ambiguous.
    let raw = std::fs::read_to_string(&config).expect("config readable");
    let raw = raw.replace(
        "  { resolver = \"custom\", paths = [\"src/**/*.component\"] },",
        "  { resolver = \"custom\", paths = [\"src/**/*.component\"] },\n  { resolver = \"dsql\", paths = [\"src/conflict/**/*.component\"] },",
    );
    std::fs::write(&config, raw).expect("config with overlapping patterns");
    let reloaded = session.files_changed("dsql/dsql.toml").await;
    assert!(
        reloaded.contains("\"result\""),
        "overlapping patterns without overlapping files stay clean, got {reloaded}"
    );

    std::fs::create_dir_all(session.root.join("src/conflict")).expect("sub dir");
    std::fs::write(
        session.root.join("src/conflict/both.component"),
        "query Both {\n  kind_type {\n    kind\n  }\n}\n",
    )
    .expect("ambiguous doc");
    let ambiguous = session.files_changed("src/conflict/both.component").await;
    assert!(
        ambiguous.contains("\"ProjectLoadFailed\"")
            && ambiguous.contains("is assigned to resolver"),
        "warm ambiguity matches the cold loader error, got {ambiguous}"
    );

    // Unseal for cleanup.
    let mut permissions = std::fs::metadata(&sealed).expect("meta").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&sealed, permissions).expect("unseal");
}
