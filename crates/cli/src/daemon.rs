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
struct DaemonGenerationErrorResponse {
    id: u64,
    error: DaemonGenerationErrorPayload,
}

#[derive(Clone, Debug, Facet)]
#[facet(rename_all = "camelCase")]
struct DaemonGenerationErrorPayload {
    message: String,
    diagnostics: Vec<DaemonDiagnostic>,
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
        let response = handle_line(&line).await;
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

async fn handle_line(line: &str) -> String {
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
                .await
            {
                Ok(artifacts) => success_response(request.id, &artifacts),
                Err(dsql_generate::GenerateError::LanguageDiagnostics { diagnostics }) => {
                    generation_error_response(request.id, diagnostics)
                }
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
    diagnostics: Vec<dsql_generate::LanguageDiagnostic>,
) -> String {
    let diagnostics = diagnostics
        .into_iter()
        .map(|diagnostic| {
            let embedded_range = DaemonRange {
                start: diagnostic.embedded_range.start,
                end: diagnostic.embedded_range.end,
            };
            DaemonDiagnostic {
                file: diagnostic
                    .path
                    .as_ref()
                    .unwrap_or(&diagnostic.physical_document.0)
                    .display()
                    .to_string(),
                range: DaemonRange {
                    start: diagnostic.range.start,
                    end: diagnostic.range.end,
                },
                embedded_range,
                source_offset: diagnostic.source_offset,
                severity: format!("{:?}", diagnostic.diagnostic.severity),
                source: format!("{:?}", diagnostic.diagnostic.source),
                code: format!("{:?}", diagnostic.diagnostic.code),
                message: diagnostic.diagnostic.message,
            }
        })
        .collect::<Vec<_>>();
    let response = DaemonGenerationErrorResponse {
        id,
        error: DaemonGenerationErrorPayload {
            message: "cannot generate while diagnostics contain errors".to_string(),
            diagnostics,
        },
    };
    facet_json::to_string(&response).unwrap_or_else(|error| {
        error_response(id, &format!("failed to serialize daemon response: {error}"))
    })
}
