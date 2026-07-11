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
    next_id: i64,
}

impl Session {
    /// Starts the server against a temp copy of the scoped fixture and
    /// completes the initialize handshake.
    async fn start(test: &str) -> Session {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dsql-project/tests/it/fixture/scoped");
        let root = std::env::temp_dir().join(format!("dsql-lsp-{test}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clean stale fixture copy");
        }
        copy_tree(&source, &root);

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
            next_id: 0,
        };
        let root_uri = format!("file://{}", session.root.display());
        let id = session
            .request(
                "initialize",
                json!({
                    "processId": null,
                    "rootUri": root_uri,
                    "capabilities": {},
                    "workspaceFolders": [{"uri": root_uri, "name": "fixture"}],
                }),
            )
            .await;
        session.response(id).await;
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

    async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await;
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

/// A dsql document publishes diagnostics on open, an edit introducing an
/// unknown column publishes the error, and reverting clears it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_follow_edits() {
    let mut session = Session::start("diagnostics").await;
    let uri = session.uri("queries/frontend/titles.dsql");
    let text = session.fixture_text("queries/frontend/titles.dsql");

    session
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "dsql", "version": 1, "text": text}}),
        )
        .await;
    let clean = session.diagnostics_for(&uri).await;
    assert_eq!(clean.as_array().map(Vec::len), Some(0), "fixture is clean");

    let broken = text.replace("...TitleBits", "not_a_column");
    session
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": 2},
                "contentChanges": [{"text": broken}],
            }),
        )
        .await;
    let dirty = session.diagnostics_for(&uri).await;
    assert!(
        dirty.to_string().contains("not_a_column"),
        "the diagnostic names the unknown column, got {dirty}"
    );

    session
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": 3},
                "contentChanges": [{"text": text}],
            }),
        )
        .await;
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
    session
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": foreign, "languageId": "toml", "version": 1, "text": "x = 1"}}),
        )
        .await;

    let uri = session.uri("src/components/TitlePanel.ts");
    let text = session.fixture_text("src/components/TitlePanel.ts");
    let offset = text.find("TitleBits").expect("spread in host");
    let line = text[..offset].matches('\n').count();
    let character = offset - text[..offset].rfind('\n').map_or(0, |at| at + 1);

    session
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "typescript", "version": 1, "text": text}}),
        )
        .await;
    let id = session
        .request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character + 1},
            }),
        )
        .await;
    let response = session.response(id).await;
    let content = response["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        content.contains("TitleBits"),
        "host hover answers through the region, got {response}"
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
    session
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "dsql", "version": 1, "text": text}}),
        )
        .await;
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
    session
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": existing, "languageId": "dsql", "version": 1, "text": disk_text}}),
        )
        .await;
    session.diagnostics_for(&existing).await;
    session
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": existing, "version": 2},
                "contentChanges": [{"text": disk_text.replace("...TitleBits", "not_a_column")}],
            }),
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
