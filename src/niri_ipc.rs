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

pub async fn get_windows() -> Result<Vec<WindowInfo>, Box<dyn std::error::Error>> {
    let request = json!("Windows");

    let response = send_request(request).await?;

    if let Some(windows) = response
        .get("Ok")
        .and_then(|v| v.get("Windows"))
        .and_then(|v| v.as_array())
    {
        let mut window_list = Vec::new();
        for window in windows {
            if let (Some(id), Some(title), Some(app_id), Some(focused)) = (
                window.get("id").and_then(|v| v.as_u64()),
                window.get("title").and_then(|v| v.as_str()),
                window.get("app_id").and_then(|v| v.as_str()),
                window.get("is_focused").and_then(|v| v.as_bool()),
            ) {
                if app_id == "org.niri.dock" {
                    continue;
                }
                window_list.push(WindowInfo {
                    id,
                    title: title.to_string(),
                    app_id: app_id.to_string(),
                    focused,
                });
            }
        }
        Ok(window_list)
    } else {
        Err("Failed to parse windows response".into())
    }
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
