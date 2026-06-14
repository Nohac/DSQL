use facet::Facet;
use miette::Result;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Debug, Facet)]
struct DaemonRequest {
    id: u64,
    method: String,
    #[facet(default)]
    params: Option<DaemonParams>,
}

#[derive(Clone, Debug, Facet)]
struct DaemonParams {
    root: String,
}

#[derive(Clone, Debug, Facet)]
#[facet(rename_all = "camelCase")]
struct DaemonRange {
    start: u32,
    end: u32,
}

#[derive(Clone, Debug, Facet)]
#[facet(rename_all = "camelCase")]
struct DaemonDiagnostic {
    file: String,
    range: DaemonRange,
    embedded_range: DaemonRange,
    source_offset: u32,
    severity: String,
    source: String,
    code: String,
    message: String,
}

#[derive(Clone, Debug, Facet)]
#[facet(rename_all = "camelCase")]
struct DaemonGenerationError {
    kind: String,
    message: String,
    file: Option<String>,
    range: Option<DaemonRange>,
    embedded_range: Option<DaemonRange>,
    source_offset: Option<u32>,
    source: String,
    code: String,
}

#[derive(Clone, Debug, Facet)]
#[facet(rename_all = "camelCase")]
struct DaemonGenerationErrorResponse {
    id: u64,
    error: DaemonGenerationErrorPayload,
}

#[derive(Clone, Debug, Facet)]
#[facet(rename_all = "camelCase")]
struct DaemonGenerationErrorPayload {
    message: String,
    diagnostics: Vec<DaemonDiagnostic>,
    errors: Vec<DaemonGenerationError>,
}

pub async fn run_stdio() -> Result<()> {
    let stdin = BufReader::new(io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = io::stdout();

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| miette::miette!("failed to read daemon request: {error}"))?
    {
        let response = handle_line(&line);
        stdout
            .write_all(response.as_bytes())
            .await
            .map_err(|error| miette::miette!("failed to write daemon response: {error}"))?;
        stdout
            .write_all(b"\n")
            .await
            .map_err(|error| miette::miette!("failed to write daemon response: {error}"))?;
        stdout
            .flush()
            .await
            .map_err(|error| miette::miette!("failed to flush daemon response: {error}"))?;
    }

    Ok(())
}

fn handle_line(line: &str) -> String {
    let request = match facet_json::from_str::<DaemonRequest>(line) {
        Ok(request) => request,
        Err(error) => {
            return error_response(0, &format!("failed to parse daemon request: {error}"));
        }
    };

    match request.method.as_str() {
        "compileProject" => {
            let Some(params) = request.params else {
                return error_response(request.id, "compileProject requires params.root");
            };
            match dsql_generate::generate_project_artifacts_from(std::path::Path::new(&params.root))
            {
                Ok(artifacts) => success_response(request.id, &artifacts),
                Err(dsql_generate::GenerateError::LanguageDiagnostics {
                    diagnostics,
                    errors,
                }) => generation_error_response(request.id, diagnostics, errors),
                Err(error) => error_response(request.id, &error.to_string()),
            }
        }
        "shutdown" => success_response(request.id, &true),
        method => error_response(request.id, &format!("unknown daemon method `{method}`")),
    }
}

fn success_response<T>(id: u64, result: &T) -> String
where
    T: facet::Facet<'static>,
{
    match facet_json::to_string(result) {
        Ok(result) => format!(r#"{{"id":{id},"result":{result}}}"#),
        Err(error) => error_response(id, &format!("failed to serialize daemon response: {error}")),
    }
}

fn error_response(id: u64, message: &str) -> String {
    let message = facet_json::to_string(&message.to_string())
        .unwrap_or_else(|_| r#""failed to serialize daemon error""#.to_string());
    format!(r#"{{"id":{id},"error":{{"message":{message}}}}}"#)
}

fn generation_error_response(
    id: u64,
    diagnostics: Vec<dsql_generate::ValidationDiagnostic>,
    errors: Vec<dsql_generate::ValidationError>,
) -> String {
    let diagnostics = diagnostics
        .into_iter()
        .map(|diagnostic| {
            let start = diagnostic.source_offset + diagnostic.diagnostic.range.start;
            let end = diagnostic.source_offset + diagnostic.diagnostic.range.end;
            let embedded_range = DaemonRange {
                start: diagnostic.diagnostic.range.start,
                end: diagnostic.diagnostic.range.end,
            };
            DaemonDiagnostic {
                file: diagnostic.file.display().to_string(),
                range: DaemonRange { start, end },
                embedded_range,
                source_offset: diagnostic.source_offset,
                severity: format!("{:?}", diagnostic.diagnostic.severity),
                source: format!("{:?}", diagnostic.diagnostic.source),
                code: format!("{:?}", diagnostic.diagnostic.code),
                message: diagnostic.diagnostic.message,
            }
        })
        .collect::<Vec<_>>();
    let errors = errors
        .into_iter()
        .map(|error| {
            let embedded_range = error.range.map(|range| DaemonRange {
                start: range.start,
                end: range.end,
            });
            let range = match (error.source_offset, error.range) {
                (Some(source_offset), Some(range)) => Some(DaemonRange {
                    start: source_offset + range.start,
                    end: source_offset + range.end,
                }),
                _ => None,
            };
            DaemonGenerationError {
                kind: format!("{:?}", error.kind),
                message: error.message,
                file: error.file.map(|file| file.display().to_string()),
                range,
                embedded_range,
                source_offset: error.source_offset,
                source: "Generate".to_string(),
                code: format!("{:?}", error.kind),
            }
        })
        .collect::<Vec<_>>();
    let response = DaemonGenerationErrorResponse {
        id,
        error: DaemonGenerationErrorPayload {
            message: "cannot generate while diagnostics contain errors".to_string(),
            diagnostics,
            errors,
        },
    };
    facet_json::to_string(&response).unwrap_or_else(|error| {
        error_response(id, &format!("failed to serialize daemon response: {error}"))
    })
}
