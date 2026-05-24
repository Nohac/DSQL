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
