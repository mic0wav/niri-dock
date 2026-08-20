use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn connect() -> Result<UnixStream, Box<dyn std::error::Error>> {
    let socket_path = get_niri_socket()?;
    let stream = UnixStream::connect(&socket_path).await?;
    Ok(stream)
}

fn get_niri_socket() -> Result<PathBuf, Box<dyn std::error::Error>> {
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

pub async fn send_request(request: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let mut stream = connect().await?;

    let request_str = format!("{}\n", request.to_string());
    stream.write_all(request_str.as_bytes()).await?;

    let (reader, _) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);

    let mut response_line = String::new();
    buf_reader.read_line(&mut response_line).await?;

    let response: Value = serde_json::from_str(&response_line)?;
    Ok(response)
}

pub async fn focus_window(id: u64) -> Result<(), Box<dyn std::error::Error>> {
    let request = json!({
        "Action": {
            "FocusWindow": { "id": id }
        }
    });
    let response = send_request(request).await?;
    if response.get("Ok").is_some() {
        Ok(())
    } else {
        Err(format!("Failed to focus window: {}", response).into())
    }
}

pub async fn spawn(command: String) -> Result<(), Box<dyn std::error::Error>> {
    let request = json!({
        "Action": {
            "Spawn": { "command": ["sh", "-c", command] }
        }
    });
    let response = send_request(request).await?;
    if response.get("Ok").is_some() {
        Ok(())
    } else {
        Err(format!("Failed to spawn command: {}", response).into())
    }
}

pub async fn event_stream(
    tx: tokio::sync::mpsc::UnboundedSender<NiriEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
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

        if let Some(event) = parse_event(&value) {
            if tx.send(event).is_err() {
                break;
            }
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

    None
}

fn parse_window(w: &Value) -> Option<WindowInfo> {
    let id = w.get("id")?.as_u64()?;
    let title = w.get("title")?.as_str()?.to_string();
    let app_id = w.get("app_id")?.as_str()?.to_string();
    let focused = w.get("is_focused")?.as_bool()?;

    if app_id == "org.niri.dock" {
        return None;
    }

    Some(WindowInfo { id, title, app_id, focused })
}
