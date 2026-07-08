//! The dsql language server binary.

#[tokio::main]
async fn main() {
    dsql_lsp::run_stdio().await;
}
