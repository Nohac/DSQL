//! Language Server Protocol adapter over the dsql bowl.

mod position;
mod server;

pub use server::run_stdio;

/// Serves the language server over arbitrary transports — the in-process
/// test harness drives it through a duplex pipe exactly as an editor
/// drives stdio.
pub async fn serve<I, O>(input: I, output: O)
where
    I: tokio::io::AsyncRead + Unpin,
    O: tokio::io::AsyncWrite,
{
    server::serve(input, output).await;
}
