use tower_lsp_server::Client;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeActionResponse,
    Command, ExecuteCommandOptions, ExecuteCommandParams, LSPAny, MessageType, ServerCapabilities,
};
use tracing::{info, warn};

use dsql_core::debug::source_allocations::{SourceAllocationStats, source_allocation_stats};

const SOURCE_ALLOCATION_STATS_COMMAND: &str = "dsql.sourceAllocationStats";

pub(crate) fn configure_capabilities(capabilities: &mut ServerCapabilities) {
    capabilities.execute_command_provider = Some(ExecuteCommandOptions {
        commands: vec![SOURCE_ALLOCATION_STATS_COMMAND.to_string()],
        ..ExecuteCommandOptions::default()
    });
    capabilities.code_action_provider = Some(CodeActionProviderCapability::Simple(true));
}

pub(crate) async fn code_action(_: CodeActionParams) -> Result<Option<CodeActionResponse>> {
    Ok(Some(vec![CodeActionOrCommand::Command(Command::new(
        "Show DSQL source allocation stats".to_string(),
        SOURCE_ALLOCATION_STATS_COMMAND.to_string(),
        None,
    ))]))
}

pub(crate) async fn execute_command(
    client: &Client,
    params: ExecuteCommandParams,
) -> Result<Option<LSPAny>> {
    if params.command != SOURCE_ALLOCATION_STATS_COMMAND {
        warn!(command = %params.command, "unknown executeCommand request");
        return Ok(None);
    }

    let stats = source_allocation_stats();
    let message = format_source_allocation_stats(stats);
    let result = source_allocation_stats_result(&stats);
    info!(%message, "source allocation stats");
    client
        .show_message(MessageType::INFO, message.clone())
        .await;
    client.log_message(MessageType::INFO, message).await;
    Ok(result)
}

fn source_allocation_stats_result(stats: &SourceAllocationStats) -> Option<LSPAny> {
    facet_json::to_string(stats)
        .ok()
        .and_then(|json| json.parse::<LSPAny>().ok())
}

fn format_source_allocation_stats(stats: SourceAllocationStats) -> String {
    format!(
        concat!(
            "dsql source allocations: ",
            "documents_from_string={} ({} bytes), ",
            "documents_from_rope={} ({} bytes), ",
            "full_text_materializations={} ({} bytes), ",
            "range_text_materializations={} ({} bytes), ",
            "region_rope_materializations={} ({} bytes)"
        ),
        stats.documents_from_string,
        stats.documents_from_string_bytes,
        stats.documents_from_rope,
        stats.documents_from_rope_bytes,
        stats.full_text_materializations,
        stats.full_text_materialization_bytes,
        stats.range_text_materializations,
        stats.range_text_materialization_bytes,
        stats.region_rope_materializations,
        stats.region_rope_materialization_bytes
    )
}
