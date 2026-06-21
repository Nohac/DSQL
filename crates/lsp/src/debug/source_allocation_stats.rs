use tower_lsp_server::Client;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    CodeActionParams, CodeActionResponse, ExecuteCommandParams, LSPAny, ServerCapabilities,
};

pub(crate) fn configure_capabilities(_: &mut ServerCapabilities) {}

pub(crate) async fn code_action(_: CodeActionParams) -> Result<Option<CodeActionResponse>> {
    Ok(None)
}

pub(crate) async fn execute_command(_: &Client, _: ExecuteCommandParams) -> Result<Option<LSPAny>> {
    Ok(None)
}
