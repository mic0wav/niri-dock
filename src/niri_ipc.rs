use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

pub const APP_ID: &str = "org.niri.dock";

pub async fn connect() -> Result<UnixStream, Box<dyn std::error::Error + Send + Sync>> {
    let socket_path = get_niri_socket()?;
    let stream = UnixStream::connect(&socket_path).await?;
    Ok(stream)
}

fn get_niri_socket() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(socket) = std::env::var("NIRI_SOCKET") {
        Ok(PathBuf::from(socket))
    } else {
        Err("NIRI_SOCKET not set — is this running inside a niri session?".into())
    }
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub focused: bool,
}

#[derive(Debug, Clone)]
pub enum NiriEvent {
    WindowsChanged(Vec<WindowInfo>),
    WindowOpenedOrChanged(WindowInfo),
    WindowClosed(u64),
    WindowFocusChanged(Option<u64>),
}

type ReplyTx = oneshot::Sender<Result<Value, String>>;

fn request_tx() -> &'static mpsc::UnboundedSender<(Value, ReplyTx)> {
    static REQUEST_TX: OnceLock<mpsc::UnboundedSender<(Value, ReplyTx)>> = OnceLock::new();
    REQUEST_TX.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        crate::runtime().spawn(request_worker(rx));
        tx
    })
}

async fn request_worker(mut rx: mpsc::UnboundedReceiver<(Value, ReplyTx)>) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    loop {
        let stream = match connect().await {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to connect to niri for requests: {e}, retrying in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = std::cmp::min(backoff * 2, max_backoff);
                continue;
            }
        };
        backoff = Duration::from_secs(1);

        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        while let Some((request, reply_tx)) = rx.recv().await {
            let result = send_on_connection(&mut writer, &mut lines, request).await;
            let failed = result.is_err();
            let _ = reply_tx.send(result);

            if failed {
                break;
            }
        }

        if rx.is_closed() {
            return;
        }
    }
}

async fn send_on_connection(
    writer: &mut OwnedWriteHalf,
    lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
    request: Value,
) -> Result<Value, String> {
    let mut request_str = request.to_string();
    request_str.push('\n');

    if let Err(e) = writer.write_all(request_str.as_bytes()).await {
        return Err(format!("write failed: {e}"));
    }

    match tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await {
        Ok(Ok(Some(line))) => {
            serde_json::from_str::<Value>(&line).map_err(|e| format!("bad JSON from niri: {e}"))
        }
        Ok(Ok(None)) => Err("niri closed the connection".to_string()),
        Ok(Err(e)) => Err(format!("read failed: {e}")),
        Err(_) => Err("timed out waiting for niri response".to_string()),
    }
}

pub async fn send_request(request: Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let (reply_tx, reply_rx) = oneshot::channel();
    request_tx()
        .send((request, reply_tx))
        .map_err(|_| "niri request worker is not running")?;

    match reply_rx.await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err("niri request worker dropped the reply".into()),
    }
}

pub async fn focus_window(id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = json!({ "Action": { "FocusWindow": { "id": id } } });
    let response = send_request(request).await?;
    if response.get("Ok").is_some() {
        Ok(())
    } else {
        Err(format!("Failed to focus window: {}", response).into())
    }
}

pub async fn spawn(command: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = json!({ "Action": { "Spawn": { "command": ["sh", "-c", command] } } });
    let response = send_request(request).await?;
    if response.get("Ok").is_some() {
        Ok(())
    } else {
        Err(format!("Failed to spawn command: {}", response).into())
    }
}

pub async fn event_stream(
    tx: mpsc::UnboundedSender<NiriEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = connect().await?;
    stream.write_all(b"\"EventStream\"\n").await?;

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    lines.next_line().await?;

    while let Some(line) = lines.next_line().await? {
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Failed to parse event line: {e}");
                continue;
            }
        };

        if let Some(event) = parse_event(&value) && tx.send(event).is_err() {
            break;
        }
    }

    Ok(())
}

fn parse_event(v: &Value) -> Option<NiriEvent> {
    if let Some(windows) = v
        .get("WindowsChanged")
        .and_then(|e| e.get("windows"))
        .and_then(|w| w.as_array())
    {
        return Some(NiriEvent::WindowsChanged(
            windows.iter().filter_map(parse_window).collect(),
        ));
    }

    if let Some(w) = v.get("WindowOpenedOrChanged").and_then(|e| e.get("window")) {
        return parse_window(w).map(NiriEvent::WindowOpenedOrChanged);
    }

    if let Some(id) = v
        .get("WindowClosed")
        .and_then(|e| e.get("id"))
        .and_then(|i| i.as_u64())
    {
        return Some(NiriEvent::WindowClosed(id));
    }

    if let Some(e) = v.get("WindowFocusChanged") {
        let id = e.get("id").and_then(|i| i.as_u64());
        return Some(NiriEvent::WindowFocusChanged(id));
    }

    log::debug!("Unrecognized niri event, ignoring: {v}");
    None
}

fn parse_window(w: &Value) -> Option<WindowInfo> {
    let id = match w.get("id").and_then(|v| v.as_u64()) {
        Some(v) => v,
        None => {
            log::warn!("Window entry missing/invalid `id`: {w}");
            return None;
        }
    };
    let title = match w.get("title").and_then(|v| v.as_str()) {
        Some(v) => v.to_string(),
        None => {
            log::warn!("Window {id} missing/invalid `title`: {w}");
            return None;
        }
    };
    let app_id = match w.get("app_id").and_then(|v| v.as_str()) {
        Some(v) => v.to_string(),
        None => {
            log::warn!("Window {id} missing/invalid `app_id`: {w}");
            return None;
        }
    };
    let focused = match w.get("is_focused").and_then(|v| v.as_bool()) {
        Some(v) => v,
        None => {
            log::warn!("Window {id} missing/invalid `is_focused`: {w}");
            return None;
        }
    };

    if app_id == APP_ID {
        return None;
    }

    Some(WindowInfo { id, title, app_id, focused })
}
