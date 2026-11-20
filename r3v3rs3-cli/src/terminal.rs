//! `r3 app terminal`: an interactive shell in the container of an app over the terminal
//! WebSocket. Keys go to the server as binary messages, the output comes back as binary
//! messages, and a size change goes as a `{"cols":..,"rows":..}` text message.

use crate::api_client::{ApiClient, WebSocket};
use anyhow::Context;
use futures::{SinkExt, StreamExt};
use std::io::{IsTerminal, Read, Write};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// How often the size of the local terminal is checked.
const RESIZE_POLL: Duration = Duration::from_millis(500);

/// Keeps the local terminal in raw mode and restores it when dropped, on every exit path.
struct RawMode;

impl RawMode {
    fn enable() -> anyhow::Result<Self> {
        crossterm::terminal::enable_raw_mode()
            .context("failed to switch the terminal to raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Err(err) = crossterm::terminal::disable_raw_mode() {
            eprintln!("failed to restore the terminal: {err}");
        }
    }
}

/// Opens a shell in the container of the app `id` and connects it to this terminal.
pub async fn run(api: &ApiClient, id: &str, shell: Option<&str>) -> anyhow::Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("cols", &cols.to_string());
    query.append_pair("rows", &rows.to_string());
    if let Some(shell) = shell {
        query.append_pair("shell", shell);
    }
    let path = format!("/api/apps/{id}/terminal?{}", query.finish());
    let socket = api.open_websocket(&path).await?;
    let _raw = if std::io::stdin().is_terminal() {
        Some(RawMode::enable()?)
    } else {
        None
    };
    bridge(socket, (cols, rows)).await
}

/// Copies stdin to the socket and the socket to stdout until either side ends.
async fn bridge(socket: WebSocket, mut size: (u16, u16)) -> anyhow::Result<()> {
    let (mut sink, mut stream) = socket.split();
    let mut keys = read_stdin();
    let mut ticker = tokio::time::interval(RESIZE_POLL);
    loop {
        tokio::select! {
            message = stream.next() => {
                if !write_output(message)? {
                    return Ok(());
                }
            }
            key = keys.recv() => match key {
                Some(bytes) => sink.send(Message::Binary(bytes.into())).await?,
                None => return Ok(sink.close().await?),
            },
            _ = ticker.tick() => {
                let now = crossterm::terminal::size().unwrap_or(size);
                if now != size {
                    size = now;
                    let resize = serde_json::json!({ "cols": now.0, "rows": now.1 });
                    sink.send(Message::Text(resize.to_string().into())).await?;
                }
            }
        }
    }
}

/// Writes one server message to stdout; false when the session ended.
fn write_output(
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> anyhow::Result<bool> {
    let mut out = std::io::stdout();
    match message.transpose().context("the terminal stream broke")? {
        Some(Message::Binary(bytes)) => out.write_all(&bytes)?,
        Some(Message::Text(text)) => out.write_all(text.as_bytes())?,
        Some(Message::Close(_)) | None => return Ok(false),
        Some(_) => {}
    }
    out.flush()?;
    Ok(true)
}

/// Reads stdin on a blocking thread, because a terminal read blocks, and sends each chunk on.
fn read_stdin() -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel(64);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 4096];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Err(err) => {
                    eprintln!("failed to read the keyboard: {err}");
                    break;
                }
                Ok(n) => {
                    if sender.blocking_send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}
