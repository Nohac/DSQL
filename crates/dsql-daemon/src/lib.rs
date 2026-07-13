//! The build daemon (docs/spec/build-daemon.md): a persistent compile
//! service host build tools drive over stdio. One consumer per daemon;
//! strictly sequential request execution over a resident project bowl,
//! transactional publication through dsql-generate, callsite ranges and
//! complete diagnostic snapshots in every compile response.

mod protocol;
mod session;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use protocol::{Request, error_line};
use session::Daemon;

/// One frame from the reader task: a line, or bytes that were not UTF-8
/// (which must answer `InvalidRequest`, not be laundered into U+FFFD).
enum Frame {
    Line(String),
    NotUtf8,
}

/// Serves the daemon over stdio until the consumer disconnects.
pub async fn run_stdio() {
    serve(tokio::io::stdin(), tokio::io::stdout()).await;
}

/// Serves the protocol over any byte streams (tests drive a duplex).
///
/// EOF contract: a reader task feeds a queue; the executor finishes the
/// in-flight request to its transactional end, suppresses its response
/// once EOF is known, drops queued requests, and returns.
pub async fn serve<I, O>(input: I, output: O)
where
    I: AsyncRead + Unpin + Send + 'static,
    O: AsyncWrite + Unpin,
{
    let (lines, mut eof) = spawn_reader(input);
    let mut lines = lines;
    let mut output = output;
    let mut daemon = Daemon::new();

    while let Some(frame) = lines.recv().await {
        let line = match frame {
            Frame::Line(line) if line.is_empty() => continue,
            Frame::Line(line) => line,
            Frame::NotUtf8 => {
                let response =
                    error_line(None, "InvalidRequest", "request line is not UTF-8", "null");
                if is_eof(&mut eof) {
                    return;
                }
                let _ = output.write_all(response.as_bytes()).await;
                let _ = output.write_all(b"\n").await;
                let _ = output.flush().await;
                continue;
            }
        };
        let response = match Request::parse(&line) {
            Ok(request) => match daemon.handle(request).await {
                session::Handled::Respond(body) => body,
                session::Handled::Shutdown(body) => {
                    if !is_eof(&mut eof) {
                        let _ = output.write_all(body.as_bytes()).await;
                        let _ = output.write_all(b"\n").await;
                        let _ = output.flush().await;
                    }
                    return;
                }
            },
            Err(bad) => error_line(bad.id, "InvalidRequest", &bad.message, &bad.data),
        };
        // EOF observed while executing: finish (we did), skip the
        // response, drop whatever is queued, exit.
        if is_eof(&mut eof) {
            return;
        }
        if output.write_all(response.as_bytes()).await.is_err() {
            return;
        }
        let _ = output.write_all(b"\n").await;
        let _ = output.flush().await;
        daemon.after_respond().await;
    }
}

fn is_eof(eof: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    *eof.borrow()
}

fn spawn_reader<I>(
    input: I,
) -> (
    tokio::sync::mpsc::UnboundedReceiver<Frame>,
    tokio::sync::watch::Receiver<bool>,
)
where
    I: AsyncRead + Unpin + Send + 'static,
{
    let (line_sender, lines) = tokio::sync::mpsc::unbounded_channel();
    let (eof_sender, eof) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let mut reader = BufReader::new(input);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    // Bytes first, decoded strictly: invalid UTF-8 must
                    // answer InvalidRequest, never be laundered into
                    // replacement characters that might parse as JSON.
                    let frame = match std::str::from_utf8(&buffer) {
                        Ok(line) => Frame::Line(line.trim().to_string()),
                        Err(_) => Frame::NotUtf8,
                    };
                    if line_sender.send(frame).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = eof_sender.send(true);
    });
    (lines, eof)
}
