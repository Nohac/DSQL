//! Wire types and response composition. Requests parse in one tolerant
//! pass (all params optional) so a bad or missing field answers
//! `InvalidRequest` with the request's own id; a line that fails the
//! envelope parse answers with `id: null`.

use facet::Facet;
use facet_json::RawJson;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[facet(rename_all = "camelCase")]
#[repr(u8)]
pub enum Method {
    Initialize,
    Compile,
    FilesChanged,
    Shutdown,
}

/// A request method recovered for an error response. Known protocol
/// methods stay typed; unknown input is retained verbatim.
#[derive(Debug, Clone, Facet)]
#[facet(untagged)]
#[repr(u8)]
#[expect(
    dead_code,
    reason = "variant payloads are read through Facet reflection"
)]
pub enum RequestMethod {
    Known(Method),
    Unknown(String),
}

/// A parse failure that still answers: with the request's own id when
/// the envelope was readable, `id: null` otherwise.
#[derive(Debug)]
pub struct BadRequest {
    pub id: Option<u64>,
    pub message: String,
    pub method: Option<RequestMethod>,
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
                    method: None,
                });
            }
        };
        let id = match header.id {
            Some(id) if (1..=MAX_ID).contains(&id) => id,
            Some(id) => {
                return Err(BadRequest {
                    id: None,
                    message: format!("id {id} is outside 1..=2^53-1"),
                    method: None,
                });
            }
            None => {
                return Err(BadRequest {
                    id: None,
                    message: "request has no id".to_string(),
                    method: None,
                });
            }
        };
        let Some(method_name) = header.method else {
            return Err(BadRequest {
                id: Some(id),
                message: "request has no method".to_string(),
                method: None,
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
                    method: Some(RequestMethod::Unknown(other.to_string())),
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
                    method: Some(RequestMethod::Known(method)),
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

/// One daemon response. Exactly one of [`Self::result`] and [`Self::error`]
/// is present.
#[derive(Debug, Clone, Facet)]
pub struct WireResponse {
    pub id: Option<u64>,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub result: Option<WireResult>,
    #[facet(default, skip_serializing_if = Option::is_none)]
    pub error: Option<WireError>,
}

impl WireResponse {
    /// Constructs a successful response for `id`.
    pub fn success(id: u64, result: WireResult) -> Self {
        Self {
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    /// Constructs a failed response with the protocol's stable error shape.
    pub fn error(id: Option<u64>, kind: WireErrorKind, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(WireError::new(kind, message)),
        }
    }

    /// Serializes one line-JSON response.
    pub fn serialize(&self) -> Result<String, String> {
        facet_json::to_string(self).map_err(|error| error.to_string())
    }
}

/// Successful result payloads supported by daemon protocol version 1.
#[derive(Debug, Clone, Facet)]
#[facet(untagged)]
#[repr(u8)]
#[expect(
    dead_code,
    reason = "variant payloads are read through Facet reflection"
)]
pub enum WireResult {
    Boolean(bool),
    Initialize(InitializeResult),
    Compile(CompileResult),
}

/// One protocol-v1 failure and the only data shape legal for its code.
#[derive(Debug, Clone, Facet)]
#[facet(untagged)]
#[repr(u8)]
#[expect(
    dead_code,
    reason = "variant payloads are read through Facet reflection"
)]
pub enum WireErrorKind {
    InvalidRequest,
    InvalidRequestMethod {
        method: RequestMethod,
    },
    NotInitialized,
    AlreadyInitialized,
    UnsupportedProtocolVersion {
        #[facet(rename = "daemonVersion")]
        daemon_version: u64,
    },
    InvalidPath {
        path: String,
    },
    ProjectLoadFailed {
        #[facet(default, skip_serializing_if = Option::is_none)]
        path: Option<String>,
        message: String,
    },
    Diagnostics {
        diagnostics: Vec<WireDiagnostic>,
    },
    ArtifactCollision {
        kind: String,
        first: String,
        #[facet(rename = "firstSource")]
        first_source: String,
        second: String,
        #[facet(rename = "secondSource")]
        second_source: String,
        path: String,
    },
    AssemblyFailed {
        artifact: String,
        message: String,
    },
    PublicationLocked,
    IoPath {
        path: String,
    },
    IoAddressCollision {
        path: String,
        artifact: String,
    },
    IoUnknown,
    GeneratorFailed {
        #[facet(rename = "generationId")]
        generation_id: u64,
        #[facet(rename = "manifestPath")]
        manifest_path: String,
    },
    Internal,
}

impl WireErrorKind {
    /// Constructs either the null or method-bearing
    /// [`WireErrorCode::InvalidRequest`] payload.
    pub fn invalid_request(method: Option<RequestMethod>) -> Self {
        match method {
            Some(method) => Self::InvalidRequestMethod { method },
            None => Self::InvalidRequest,
        }
    }

    fn code(&self) -> WireErrorCode {
        match self {
            Self::InvalidRequest | Self::InvalidRequestMethod { .. } => {
                WireErrorCode::InvalidRequest
            }
            Self::NotInitialized => WireErrorCode::NotInitialized,
            Self::AlreadyInitialized => WireErrorCode::AlreadyInitialized,
            Self::UnsupportedProtocolVersion { .. } => WireErrorCode::UnsupportedProtocolVersion,
            Self::InvalidPath { .. } => WireErrorCode::InvalidPath,
            Self::ProjectLoadFailed { .. } => WireErrorCode::ProjectLoadFailed,
            Self::Diagnostics { .. } => WireErrorCode::Diagnostics,
            Self::ArtifactCollision { .. } => WireErrorCode::ArtifactCollision,
            Self::AssemblyFailed { .. } => WireErrorCode::AssemblyFailed,
            Self::PublicationLocked => WireErrorCode::PublicationLocked,
            Self::IoPath { .. } | Self::IoAddressCollision { .. } | Self::IoUnknown => {
                WireErrorCode::Io
            }
            Self::GeneratorFailed { .. } => WireErrorCode::GeneratorFailed,
            Self::Internal => WireErrorCode::Internal,
        }
    }

    fn has_data(&self) -> bool {
        !matches!(
            self,
            Self::InvalidRequest
                | Self::NotInitialized
                | Self::AlreadyInitialized
                | Self::PublicationLocked
                | Self::IoUnknown
                | Self::Internal
        )
    }
}

/// Stable machine-readable daemon error codes.
#[derive(Debug, Clone, Copy, Facet)]
#[repr(u8)]
enum WireErrorCode {
    InvalidRequest,
    NotInitialized,
    AlreadyInitialized,
    UnsupportedProtocolVersion,
    InvalidPath,
    ProjectLoadFailed,
    Diagnostics,
    ArtifactCollision,
    AssemblyFailed,
    PublicationLocked,
    Io,
    GeneratorFailed,
    Internal,
}

/// The stable error body nested in a [`WireResponse`].
#[derive(Debug, Clone, Facet)]
pub struct WireError {
    code: WireErrorCode,
    message: String,
    data: Option<WireErrorKind>,
}

impl WireError {
    /// Converts one closed protocol failure into its code/data wire pair.
    pub fn new(kind: WireErrorKind, message: impl Into<String>) -> Self {
        let code = kind.code();
        let data = kind.has_data().then_some(kind);
        Self {
            code,
            message: message.into(),
            data,
        }
    }
}

/// Canonical project paths returned by `initialize`.
#[derive(Debug, Clone, Facet)]
pub struct InitializeResult {
    #[facet(rename = "protocolVersion")]
    pub protocol_version: u64,
    #[facet(rename = "projectBase")]
    pub project_base: String,
    #[facet(rename = "configPath")]
    pub config_path: String,
    #[facet(rename = "schemaDir")]
    pub schema_dir: String,
    #[facet(rename = "buildDir")]
    pub build_dir: String,
    #[facet(rename = "generatorOutputs")]
    pub generator_outputs: Vec<String>,
    #[facet(rename = "diagnosticLevel")]
    pub diagnostic_level: String,
}

/// One compile result returned by `compile` or `filesChanged`.
#[derive(Debug, Clone, Facet)]
pub struct CompileResult {
    #[facet(rename = "generationId")]
    pub generation_id: u64,
    pub changed: bool,
    #[facet(rename = "manifestPath")]
    pub manifest_path: String,
    #[facet(rename = "currentManifestPath")]
    pub current_manifest_path: String,
    #[facet(rename = "projectContractHash")]
    pub project_contract_hash: WireHash,
    pub manifest: RawJson<'static>,
    pub artifacts: Vec<WireArtifact>,
    pub groups: Vec<WireGroup>,
    #[facet(rename = "sourceFileScopes")]
    pub source_file_scopes: Vec<WireSourceFileScope>,
    pub callsites: Vec<WireCallsite>,
    pub diagnostics: Vec<WireDiagnostic>,
}

/// A named hash used for project contracts and embedding host freshness.
#[derive(Debug, Clone, Facet)]
pub struct WireHash {
    pub algorithm: String,
    pub value: String,
}

/// One compiled artifact and its canonical metadata document.
#[derive(Debug, Clone, Facet)]
pub struct WireArtifact {
    pub id: String,
    pub kind: String,
    pub scope: String,
    pub metadata: RawJson<'static>,
}

/// One resolution-scope group and its effective artifact closure.
#[derive(Debug, Clone, Facet)]
pub struct WireGroup {
    pub name: String,
    pub imports: Vec<String>,
    #[facet(rename = "generationTarget")]
    pub generation_target: bool,
    pub artifacts: Vec<String>,
}

/// Resolution scope assigned to one source file.
#[derive(Debug, Clone, Facet)]
pub struct WireSourceFileScope {
    pub path: String,
    pub scope: String,
}

/// All embedded DSQL expressions extracted from one host file.
#[derive(Debug, Clone, Facet)]
pub struct WireCallsite {
    pub path: String,
    pub resolver: String,
    #[facet(rename = "contentHash")]
    pub content_hash: WireHash,
    pub expressions: Vec<WireExpression>,
}

/// One embedded expression's host range and resolved artifact target.
#[derive(Debug, Clone, Facet)]
pub struct WireExpression {
    pub range: WireRange,
    pub target: String,
}

/// A half-open byte range in protocol coordinates.
#[derive(Debug, Clone, Facet)]
pub struct WireRange {
    pub start: usize,
    pub end: usize,
}

/// One diagnostic in the protocol's snapshot shape (host coordinates,
/// with the embedded range when the finding sits in a region).
#[derive(Debug, Clone, Facet)]
pub struct WireDiagnostic {
    pub file: String,
    pub range: WireRange,
    #[facet(
        default,
        rename = "embeddedRange",
        skip_serializing_if = Option::is_none
    )]
    pub embedded_range: Option<WireRange>,
    pub severity: String,
    pub source: String,
    pub code: String,
    pub message: String,
}
