//! In-process LSP protocol tests: the server runs over a duplex pipe and
//! is driven with raw JSON-RPC frames, exactly as an editor drives stdio.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

struct Session {
    input: DuplexStream,
    output: DuplexStream,
    buffer: Vec<u8>,
    root: PathBuf,
    initialize: Value,
    next_id: i64,
}

impl Session {
    /// Starts the server against a temp copy of the scoped fixture and
    /// completes the initialize handshake.
    async fn start(test: &str) -> Session {
        Self::start_with_overlay(test, None).await
    }

    async fn start_with_overlay(test: &str, overlay: Option<&str>) -> Session {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped");
        let root = std::env::temp_dir().join(format!("dsql-lsp-{test}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clean stale fixture copy");
        }
        copy_tree(&source, &root);
        if let Some(overlay) = overlay {
            let path = root.join("dsql/overlays/editor.yaml");
            std::fs::create_dir_all(path.parent().expect("overlay parent"))
                .expect("overlay directory");
            std::fs::write(path, overlay).expect("overlay fixture");
            std::fs::write(
                root.join("dsql/schema/type_map.yaml"),
                "types:\n  - internal_type: int4\n    readable_type: integer\n    schema: pg_catalog\n    operations: []\n",
            )
            .expect("overlay type map");
        }

        let (client_in, server_in) = tokio::io::duplex(64 * 1024);
        let (server_out, client_out) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            dsql_lsp::serve(server_in, server_out).await;
        });

        let mut session = Session {
            input: client_in,
            output: client_out,
            buffer: Vec::new(),
            root,
            initialize: Value::Null,
            next_id: 0,
        };
        let root_uri = format!("file://{}", session.root.display());
        session.initialize = session
            .request_response(
                "initialize",
                json!({
                    "processId": null,
                    "rootUri": root_uri,
                    "capabilities": {},
                    "workspaceFolders": [{"uri": root_uri, "name": "fixture"}],
                }),
            )
            .await;
        session.notify("initialized", json!({})).await;
        session
    }

    fn uri(&self, relative: &str) -> String {
        format!("file://{}", self.root.join(relative).display())
    }

    fn fixture_text(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative)).expect("fixture file readable")
    }

    async fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).expect("message serializes");
        let frame = format!("Content-Length: {}\r\n\r\n", body.len());
        self.input.write_all(frame.as_bytes()).await.expect("frame");
        self.input.write_all(&body).await.expect("body");
    }

    async fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        id
    }

    async fn request_response(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params).await;
        self.response(id).await
    }

    async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await;
    }

    async fn open(&mut self, uri: &str, language_id: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text,
                }
            }),
        )
        .await;
    }

    async fn replace(&mut self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        )
        .await;
    }

    async fn formatting(&mut self, uri: &str) -> Value {
        self.request_response(
            "textDocument/formatting",
            json!({
                "textDocument": {"uri": uri},
                "options": {"tabSize": 2, "insertSpaces": true},
            }),
        )
        .await
    }

    async fn definition(&mut self, uri: &str, text: &str, byte_offset: usize) -> Value {
        self.request_response(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": uri},
                "position": protocol_position(text, byte_offset),
            }),
        )
        .await["result"]
            .clone()
    }

    /// Reads framed messages until the response to `id` arrives; other
    /// messages (diagnostics pushes, logs) are discarded. Bounded: a
    /// server that stops answering fails the test instead of hanging it.
    async fn response(&mut self, id: i64) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let message = self.read_message().await;
                if message.get("id").and_then(Value::as_i64) == Some(id) {
                    return message;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no response to request {id} within 30s"))
    }

    /// Reads messages until a `publishDiagnostics` for `uri` arrives,
    /// bounded like [`Session::response`].
    async fn diagnostics_for(&mut self, uri: &str) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let message = self.read_message().await;
                if message.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                    && message["params"]["uri"].as_str() == Some(uri)
                {
                    return message["params"]["diagnostics"].clone();
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no diagnostics published for {uri} within 30s"))
    }

    async fn read_message(&mut self) -> Value {
        loop {
            if let Some(header_end) = find_subsequence(&self.buffer, b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&self.buffer[..header_end]).to_string();
                let length: usize = header
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .expect("content length header")
                    .trim()
                    .parse()
                    .expect("length parses");
                let body_start = header_end + 4;
                if self.buffer.len() >= body_start + length {
                    let body: Vec<u8> = self.buffer[body_start..body_start + length].to_vec();
                    self.buffer.drain(..body_start + length);
                    return serde_json::from_slice(&body).expect("message parses");
                }
            }
            let mut chunk = [0u8; 16 * 1024];
            let read = self.output.read(&mut chunk).await.expect("server alive");
            assert!(read > 0, "server closed the pipe");
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

fn protocol_position(text: &str, byte_offset: usize) -> Value {
    let prefix = &text[..byte_offset];
    let line = prefix.matches('\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |offset| offset + 1);
    let character = text[line_start..byte_offset].encode_utf16().count();
    json!({"line": line, "character": character})
}

fn protocol_range(text: &str, start: usize, end: usize) -> Value {
    json!({
        "start": protocol_position(text, start),
        "end": protocol_position(text, end),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_definitions_target_schema_yaml() {
    let mut session = Session::start("catalog-definition").await;
    let uri = session.uri("queries/frontend/titles.dsql");
    let text = "query Titles {\n  title(limit 2) {\n    id\n    kind_type { id }\n  }\n}\n";
    session.open(&uri, "dsql", text).await;
    let diagnostics = session.diagnostics_for(&uri).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "definition fixture is clean"
    );

    let table = session
        .definition(&uri, text, text.find("title(").expect("table occurrence"))
        .await;
    let column = session
        .definition(&uri, text, text.find("id\n").expect("column occurrence"))
        .await;
    let relation = session
        .definition(
            &uri,
            text,
            text.find("kind_type").expect("relation occurrence"),
        )
        .await;
    let schema_uri = session.uri("dsql/schema/public/title.yaml");
    assert_eq!(table["uri"].as_str(), Some(schema_uri.as_str()));
    assert_eq!(
        table["range"],
        json!({
            "start": {"line": 2, "character": 0},
            "end": {"line": 2, "character": 0},
        })
    );
    assert_eq!(column["uri"].as_str(), Some(schema_uri.as_str()));
    assert_eq!(
        column["range"],
        json!({
            "start": {"line": 5, "character": 0},
            "end": {"line": 5, "character": 0},
        })
    );
    assert_eq!(relation["uri"].as_str(), Some(schema_uri.as_str()));
    assert_eq!(
        relation["range"],
        json!({
            "start": {"line": 60, "character": 0},
            "end": {"line": 60, "character": 0},
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authored_relationship_definitions_target_overlay_yaml() {
    let overlay = indoc::indoc! {r#"
        version: 1
        objects:
          - target:
              schema: public
              name: title
            description: Titles exposed by the read model.
            columns:
              - name: id
                description: Stable title identifier.
              - name: phonetic_code
                hidden: true
            relationships:
              - name: kind
                target:
                  schema: public
                  name: kind_type
                columns:
                  - local: kind_id
                    target: id
            hide:
              relationships:
                - target:
                    schema: public
                    name: kind_type
                  selector: kind_id
                  direction: referencing
    "#};
    let mut session = Session::start_with_overlay("overlay-definition", Some(overlay)).await;
    let uri = session.uri("queries/frontend/titles.dsql");
    let text = "query Titles {\n  title(limit 2) {\n    id\n    kind { id }\n  }\n}\n";
    session.open(&uri, "dsql", text).await;
    let diagnostics = session.diagnostics_for(&uri).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "authored relationship fixture is clean: {diagnostics}"
    );

    let definition = session
        .definition(
            &uri,
            text,
            text.find("kind {").expect("relation occurrence"),
        )
        .await;
    let table_definition = session
        .definition(&uri, text, text.find("title").expect("table occurrence"))
        .await;
    let column_definition = session
        .definition(&uri, text, text.find("id").expect("column occurrence"))
        .await;
    let overlay_uri = session.uri("dsql/overlays/editor.yaml");
    assert_eq!(definition["uri"].as_str(), Some(overlay_uri.as_str()));
    assert_eq!(
        definition["range"],
        json!({
            "start": {"line": 12, "character": 0},
            "end": {"line": 12, "character": 0},
        })
    );
    assert_eq!(table_definition["uri"].as_str(), Some(overlay_uri.as_str()));
    assert_eq!(
        table_definition["range"],
        json!({
            "start": {"line": 4, "character": 0},
            "end": {"line": 4, "character": 0},
        })
    );
    assert_eq!(
        column_definition["uri"].as_str(),
        Some(overlay_uri.as_str())
    );
    assert_eq!(
        column_definition["range"],
        json!({
            "start": {"line": 7, "character": 0},
            "end": {"line": 7, "character": 0},
        })
    );

    let completion_text = "query VisibleFields {\n  title(limit 1) {\n    \n  }\n}\n";
    session.replace(&uri, 2, completion_text).await;
    let completion = session
        .request_response(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 2, "character": 4},
            }),
        )
        .await;
    let labels = completion["result"]
        .as_array()
        .expect("completion answers")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"kind"), "authored relation is offered");
    assert!(
        !labels.contains(&"phonetic_code") && !labels.contains(&"kind_type"),
        "hidden columns and provider relations are absent: {completion}"
    );

    let hidden_text = "query HiddenField {\n  title(limit 1) {\n    phonetic_code\n  }\n}\n";
    session.replace(&uri, 3, hidden_text).await;
    let hover = session
        .request_response(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": protocol_position(
                    hidden_text,
                    hidden_text.find("phonetic_code").expect("hidden field") + 1
                ),
            }),
        )
        .await;
    assert!(
        hover["result"].is_null(),
        "hidden catalog fields do not produce hover information: {hover}"
    );
}

/// A dsql document publishes diagnostics on open, an edit introducing an
/// unknown column publishes the error, and reverting clears it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_follow_edits() {
    let mut session = Session::start("diagnostics").await;
    let uri = session.uri("queries/frontend/titles.dsql");
    let text = session.fixture_text("queries/frontend/titles.dsql");

    session.open(&uri, "dsql", &text).await;
    let clean = session.diagnostics_for(&uri).await;
    assert_eq!(clean.as_array().map(Vec::len), Some(0), "fixture is clean");

    let broken = text.replace("...TitleBits", "not_a_column");
    session.replace(&uri, 2, &broken).await;
    let dirty = session.diagnostics_for(&uri).await;
    assert!(
        dirty.to_string().contains("not_a_column"),
        "the diagnostic names the unknown column, got {dirty}"
    );

    session.replace(&uri, 3, &text).await;
    let clean_again = session.diagnostics_for(&uri).await;
    assert_eq!(
        clean_again.as_array().map(Vec::len),
        Some(0),
        "reverting must clear the diagnostic"
    );
}

/// Host files answer hover at host coordinates through their embedded
/// regions; files the server does not own are ignored without breaking
/// the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hosts_hover_and_foreign_files_are_ignored() {
    let mut session = Session::start("hosts").await;

    // A file type the server does not own: opening it must not wedge
    // anything (the next interactions still answer).
    let foreign = session.uri("dsql/dsql.toml");
    session.open(&foreign, "toml", "x = 1").await;

    // Explicit editor language configuration preserves standalone DSQL
    // editing outside every project document pattern, without a suffix rule.
    let scratch = session.uri("scratch.loose");
    let scratch_text = "query Scratch { title(limit 1){ id } }";
    session.open(&scratch, "dsql", scratch_text).await;
    let response = session.formatting(&scratch).await;
    assert!(
        response["result"]
            .as_array()
            .is_some_and(|edits| !edits.is_empty()),
        "languageId=dsql enables unmatched standalone editing, got {response}"
    );

    let uri = session.uri("src/components/TitlePanel.ts");
    let text = session.fixture_text("src/components/TitlePanel.ts");
    let offset = text.find("TitleBits").expect("spread in host");
    let position = protocol_position(&text, offset + 1);

    session.open(&uri, "typescript", &text).await;
    let response = session
        .request_response(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": position,
            }),
        )
        .await;
    let content = response["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        content.contains("TitleBits"),
        "host hover answers through the region, got {response}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semantic_tokens_advertise_and_encode_the_host_wire_contract() {
    let mut session = Session::start("semantic-tokens").await;
    assert_eq!(
        session.initialize["result"]["capabilities"]["semanticTokensProvider"]["legend"],
        json!({
            "tokenTypes": ["namespace", "class", "relation", "property", "fragment", "alias", "policy"],
            "tokenModifiers": [],
        })
    );

    let uri = session.uri("src/components/SemanticTokens.ts");
    let text = indoc::indoc! {r#"
        const marker = "😀";
        export const query = dsql(`
        query Tokens {
          title(where .title == "😀" limit 1) { movie_id: id }
        }
        `);
    "#};
    session.open(&uri, "typescript", text).await;
    let response = session
        .request_response(
            "textDocument/semanticTokens/full",
            json!({"textDocument": {"uri": uri}}),
        )
        .await;

    assert_eq!(
        response["result"]["data"],
        json!([
            2, 6, 6, 4, 0, 1, 2, 5, 1, 0, 0, 13, 5, 3, 0, 0, 25, 8, 5, 0, 0, 10, 2, 3, 0,
        ]),
        "tokens use host lines, UTF-16 columns, and LSP delta encoding"
    );
}

/// Accepting a completion mid-identifier replaces the identifier: items
/// carry a text edit spanning the word under the cursor, in the buffer's
/// own coordinates — including host coordinates inside embedded regions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_edits_replace_the_word_under_the_cursor() {
    let mut session = Session::start("completion-edits").await;

    // Plain document: cursor inside `kin` on line 2; the edit must span
    // exactly that word.
    let uri = session.uri("queries/frontend/partial.dsql");
    let text = "query Partial {\n  title(limit 1) {\n    kin\n  }\n}\n";
    session.open(&uri, "dsql", text).await;
    let response = session
        .request_response(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 2, "character": 6},
            }),
        )
        .await;
    let items = response["result"].as_array().expect("completion answers");
    let kind_id = items
        .iter()
        .find(|item| item["label"] == "kind_id")
        .unwrap_or_else(|| panic!("kind_id offered, got {response}"));
    assert_eq!(
        kind_id["textEdit"],
        json!({
            "range": {
                "start": {"line": 2, "character": 4},
                "end": {"line": 2, "character": 7},
            },
            "newText": "kind_id",
        }),
        "the edit replaces `kin`"
    );
    assert_eq!(
        kind_id["documentation"],
        json!({
            "kind": "markdown",
            "value": "Classification assigned to the title.",
        }),
        "catalog comments become completion documentation"
    );

    // Embedded region: cursor inside `kind` (the nested `kind` column in
    // the template) — the edit range must be in host coordinates.
    let host_uri = session.uri("src/components/TitlePanel.ts");
    let host_text = session.fixture_text("src/components/TitlePanel.ts");
    let offset = host_text.find("      kind\n").expect("nested column") + 6;
    let line = host_text[..offset].matches('\n').count();
    session.open(&host_uri, "typescript", &host_text).await;
    let response = session
        .request_response(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": host_uri},
                "position": {"line": line, "character": 8},
            }),
        )
        .await;
    let items = response["result"]
        .as_array()
        .expect("host completion answers");
    let kind = items
        .iter()
        .find(|item| item["label"] == "kind")
        .unwrap_or_else(|| panic!("kind offered through the region, got {response}"));
    assert_eq!(
        kind["textEdit"]["range"],
        json!({
            "start": {"line": line, "character": 6},
            "end": {"line": line, "character": 10},
        }),
        "the edit spans `kind` at host coordinates"
    );
}

/// A newly created file in a configured scope resolves that scope's
/// imports (the default-scope bug would leave `TitleBits` unknown), and
/// closing an edited buffer restores the disk revision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_files_join_their_configured_scope_and_close_restores_disk() {
    let mut session = Session::start("scopes").await;

    // A brand-new file under queries/frontend: the frontend scope imports
    // shared, so the shared fragment must resolve.
    let uri = session.uri("queries/frontend/fresh.dsql");
    let text = "query Fresh {\n  title(limit 1) {\n    ...TitleBits\n  }\n}\n";
    session.open(&uri, "dsql", text).await;
    let fresh = session.diagnostics_for(&uri).await;
    assert_eq!(
        fresh.as_array().map(Vec::len),
        Some(0),
        "configured-scope imports must resolve, got {fresh}"
    );

    // Break an existing on-disk file in the editor, then close it: the
    // disk revision is authoritative again and the diagnostic retires.
    let existing = session.uri("queries/frontend/titles.dsql");
    let disk_text = session.fixture_text("queries/frontend/titles.dsql");
    session.open(&existing, "dsql", &disk_text).await;
    session.diagnostics_for(&existing).await;
    session
        .replace(
            &existing,
            2,
            &disk_text.replace("...TitleBits", "not_a_column"),
        )
        .await;
    let dirty = session.diagnostics_for(&existing).await;
    assert!(dirty.to_string().contains("not_a_column"), "got {dirty}");

    session
        .notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": existing}}),
        )
        .await;
    let restored = session.diagnostics_for(&existing).await;
    assert_eq!(
        restored.as_array().map(Vec::len),
        Some(0),
        "closing restores the clean disk revision, got {restored}"
    );
}

/// Formatting follows bowl-carried document and region facts: plain dsql
/// replaces the whole buffer, while a host receives one edit per clean,
/// changed region in host coordinates. Broken regions and host text are
/// preserved, and a second request against the formatted revision is a
/// no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn formatting_follows_dsql_documents_and_embedded_regions() {
    let mut session = Session::start("formatting").await;

    let plain_uri = session.uri("queries/frontend/formatting.dsql");
    let plain = "query Plain { title(limit 1){ id } }";
    session.open(&plain_uri, "dsql", plain).await;
    let response = session.formatting(&plain_uri).await;
    assert_eq!(
        response["result"],
        json!([{
            "range": protocol_range(plain, 0, plain.len()),
            "newText": "query Plain {\n  title(limit 1) {\n    id\n  }\n}\n",
        }]),
        "plain dsql keeps whole-document formatting, got {response}"
    );

    let inline = "query Inline { title(limit 1){id} }";
    let indented =
        "\n    query Indented { title(limit 1){title} }\n\n    fragment Extra on title { id }\n  ";
    let broken = "query Broken { title(where }";
    let empty = " \n  ";
    let host = format!(
        "const marker = \"🎬\"; export const inline = dsql(`{inline}`);\n\
         export const indented = dsql(`{indented}`);\n\
         const keep = \"typescript\";\n\
         export const broken = dsql(`{broken}`);\n\
         export const empty = dsql(`{empty}`);\n"
    );
    let inline_start = host.find(inline).expect("inline region");
    let indented_start = host.find(indented).expect("indented region");
    let formatted_inline = "query Inline {\n  title(limit 1) {\n    id\n  }\n}";
    let formatted_indented = "\n    query Indented {\n      title(limit 1) {\n        title\n      }\n    }\n\n    fragment Extra on title {\n      id\n    }\n  ";

    let host_uri = session.uri("src/components/Formatting.component");
    session.open(&host_uri, "typescript", &host).await;
    let response = session.formatting(&host_uri).await;
    assert_eq!(
        response["result"],
        json!([
            {
                "range": protocol_range(&host, inline_start, inline_start + inline.len()),
                "newText": formatted_inline,
            },
            {
                "range": protocol_range(
                    &host,
                    indented_start,
                    indented_start + indented.len()
                ),
                "newText": formatted_indented,
            }
        ]),
        "only clean changed regions edit the host, got {response}"
    );

    let mut formatted_host = host.clone();
    formatted_host.replace_range(
        indented_start..indented_start + indented.len(),
        formatted_indented,
    );
    formatted_host.replace_range(inline_start..inline_start + inline.len(), formatted_inline);
    assert!(
        formatted_host.contains("const keep = \"typescript\";")
            && formatted_host.contains(broken)
            && formatted_host.contains(&format!("dsql(`{empty}`)")),
        "format edits preserve host code and non-formatting sibling regions"
    );

    session.replace(&host_uri, 2, &formatted_host).await;
    let response = session.formatting(&host_uri).await;
    assert_eq!(
        response["result"],
        Value::Null,
        "formatting the updated host is idempotent, got {response}"
    );

    let empty_host_uri = session.uri("src/components/NoDsql.tsx");
    session
        .open(
            &empty_host_uri,
            "typescriptreact",
            "export const value = 'not dsql';\n",
        )
        .await;
    let response = session.formatting(&empty_host_uri).await;
    assert_eq!(
        response["result"],
        Value::Null,
        "a host without derived dsql regions has nothing to format"
    );
}
