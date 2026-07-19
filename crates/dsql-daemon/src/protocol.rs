//! Wire types and response composition. Requests parse in one tolerant
//! pass (all params optional) so a bad or missing field answers
//! `InvalidRequest` with the request's own id; a line that fails the
//! envelope parse answers with `id: null`.

use facet::Facet;

pub const PROTOCOL_VERSION: u64 = 1;

/// Upper bound for interoperable JSON integers (2^53 - 1).
pub const MAX_ID: u64 = 9_007_199_254_740_991;

#[derive(Debug, Facet)]
pub struct RequestEnvelope {
    #[facet(default)]
    pub id: Option<u64>,
    #[facet(default)]
    pub method: Option<String>,
    #[facet(default)]
    pub params: Option<Params>,
}

#[derive(Debug, Default, Facet)]
pub struct Params {
    #[facet(default, rename = "protocolVersion")]
    pub protocol_version: Option<u64>,
    #[facet(default)]
    pub root: Option<String>,
    #[facet(default, rename = "excludeRoots")]
    pub exclude_roots: Option<Vec<String>>,
    #[facet(default, rename = "diagnosticLevel")]
    pub diagnostic_level: Option<String>,
    #[facet(default)]
    pub paths: Option<Vec<String>>,
}

/// Lowest diagnostic severity included in this daemon session's snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Info,
}

impl DiagnosticLevel {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("info") {
            "error" => Ok(Self::Error),
            "warning" => Ok(Self::Warning),
            "info" => Ok(Self::Info),
            value => Err(format!(
                "initialize params.diagnosticLevel must be `error`, `warning`, or `info`, got `{value}`"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }

    pub fn includes(self, severity: &str) -> bool {
        match self {
            Self::Error => severity == "Error",
            Self::Warning => matches!(severity, "Error" | "Warning"),
            Self::Info => true,
        }
    }
}

#[derive(Debug)]
pub struct Request {
    pub id: u64,
    pub method: Method,
    pub params: Params,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Initialize,
    Compile,
    FilesChanged,
    Shutdown,
}

/// A parse failure that still answers: with the request's own id when
/// the envelope was readable, `id: null` otherwise.
#[derive(Debug)]
pub struct BadRequest {
    pub id: Option<u64>,
    pub message: String,
    pub data: String,
}

/// Best-effort id recovery for lines whose full envelope failed (bad
/// param types, unexpected shapes): a minimal header parse.
#[derive(Facet)]
struct IdHeader {
    #[facet(default)]
    id: Option<u64>,
}

/// Stage-1 header: id and method only (unknown fields — `params`
/// included — are ignored by the parser), so a readable request keeps
/// its id even when its params are the wrong shape for the method.
#[derive(Facet)]
struct Header {
    #[facet(default)]
    id: Option<u64>,
    #[facet(default)]
    method: Option<String>,
}

impl Request {
    /// Two-stage parse: the header first (id + method), the typed
    /// envelope second; only a line whose header is unreadable answers
    /// with `id: null`.
    pub fn parse(line: &str) -> Result<Request, BadRequest> {
        let header: Header = match facet_json::from_str(line) {
            Ok(header) => header,
            Err(error) => {
                let id = facet_json::from_str::<IdHeader>(line)
                    .ok()
                    .and_then(|header| header.id)
                    .filter(|id| *id >= 1 && *id <= MAX_ID);
                return Err(BadRequest {
                    id,
                    message: format!("unparseable request: {error}"),
                    data: "null".to_string(),
                });
            }
        };
        let id = match header.id {
            Some(id) if (1..=MAX_ID).contains(&id) => id,
            Some(id) => {
                return Err(BadRequest {
                    id: None,
                    message: format!("id {id} is outside 1..=2^53-1"),
                    data: "null".to_string(),
                });
            }
            None => {
                return Err(BadRequest {
                    id: None,
                    message: "request has no id".to_string(),
                    data: "null".to_string(),
                });
            }
        };
        let Some(method_name) = header.method else {
            return Err(BadRequest {
                id: Some(id),
                message: "request has no method".to_string(),
                data: "null".to_string(),
            });
        };
        let method = match method_name.as_str() {
            "initialize" => Method::Initialize,
            "compile" => Method::Compile,
            "filesChanged" => Method::FilesChanged,
            "shutdown" => Method::Shutdown,
            other => {
                return Err(BadRequest {
                    id: Some(id),
                    message: format!("unknown daemon method `{other}`"),
                    data: format!("{{\"method\":{}}}", json_string(other)),
                });
            }
        };
        // Stage 2: the typed envelope. A shape mismatch keeps the id AND
        // names the method.
        let envelope: RequestEnvelope = match facet_json::from_str(line) {
            Ok(envelope) => envelope,
            Err(error) => {
                return Err(BadRequest {
                    id: Some(id),
                    message: format!("invalid params for `{method_name}`: {error}"),
                    data: format!("{{\"method\":{}}}", json_string(&method_name)),
                });
            }
        };
        Ok(Request {
            id,
            method,
            params: envelope.params.unwrap_or_default(),
        })
    }
}

/// A JSON string literal (escaping through facet).
pub fn json_string(value: &str) -> String {
    facet_json::to_string(&value.to_string()).unwrap_or_else(|_| "\"\"".to_string())
}

/// `{"id":N,"result":<body>}` — `body` is already JSON.
pub fn result_line(id: u64, body: &str) -> String {
    format!("{{\"id\":{id},\"result\":{body}}}")
}

/// `{"id":N|null,"error":{"code":…,"message":…,"data":…}}`.
pub fn error_line(id: Option<u64>, code: &str, message: &str, data: &str) -> String {
    let id = id.map_or("null".to_string(), |id| id.to_string());
    format!(
        "{{\"id\":{id},\"error\":{{\"code\":{},\"message\":{},\"data\":{data}}}}}",
        json_string(code),
        json_string(message),
    )
}

/// One diagnostic in the protocol's snapshot shape (host coordinates,
/// with the embedded range when the finding sits in a region).
#[derive(Debug, Clone)]
pub struct WireDiagnostic {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub embedded: Option<(usize, usize)>,
    pub severity: String,
    pub source: String,
    pub code: String,
    pub message: String,
}

impl WireDiagnostic {
    pub fn render(&self) -> String {
        let embedded = self.embedded.map_or(String::new(), |(start, end)| {
            format!("\"embeddedRange\":{{\"start\":{start},\"end\":{end}}},")
        });
        format!(
            "{{\"file\":{},\"range\":{{\"start\":{},\"end\":{}}},{embedded}\"severity\":{},\"source\":{},\"code\":{},\"message\":{}}}",
            json_string(&self.file),
            self.start,
            self.end,
            json_string(&self.severity),
            json_string(&self.source),
            json_string(&self.code),
            json_string(&self.message),
        )
    }
}

pub fn render_diagnostics(diagnostics: &[WireDiagnostic]) -> String {
    let rendered: Vec<String> = diagnostics.iter().map(WireDiagnostic::render).collect();
    format!("[{}]", rendered.join(","))
}
