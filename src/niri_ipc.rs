use niri_ipc_types::{Action, Event, Reply, Request};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
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

type ReplyTx = oneshot::Sender<Result<Reply, String>>;

fn request_tx() -> &'static mpsc::UnboundedSender<(Request, ReplyTx)> {
    static REQUEST_TX: OnceLock<mpsc::UnboundedSender<(Request, ReplyTx)>> = OnceLock::new();
    REQUEST_TX.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        crate::runtime().spawn(request_worker(rx));
        tx
    })
}

async fn request_worker(mut rx: mpsc::UnboundedReceiver<(Request, ReplyTx)>) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    loop {
        let stream = match connect().await {
            Ok(s) => s,
            Err(e) => {
                log::error!("REQ: connect failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = std::cmp::min(backoff * 2, max_backoff);
                continue;
            }
        };

        let started = std::time::Instant::now();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        loop {
            let Some((request, reply_tx)) = rx.recv().await else {
                return;
            };

            let result = send_on_connection(&mut writer, &mut lines, request).await;
            let failed = result.is_err();
            if reply_tx.send(result).is_err() {
                log::error!("REQ: caller already gave up waiting for this reply");
            }

            if failed {
                log::warn!("REQ: connection considered dead, reconnecting");
                break;
            }
        }

        backoff = if started.elapsed() > Duration::from_secs(10) {
            Duration::from_secs(1)
        } else {
            std::cmp::min(backoff * 2, max_backoff)
        };
        tokio::time::sleep(backoff).await;
    }
}

async fn send_on_connection(
    writer: &mut OwnedWriteHalf,
    lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
    request: Request,
) -> Result<Reply, String> {
    let mut request_str =
        serde_json::to_string(&request).map_err(|e| format!("failed to serialize request: {e}"))?;
    request_str.push('\n');

    if let Err(e) = writer.write_all(request_str.as_bytes()).await {
        return Err(format!("write failed: {e}"));
    }

    match tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await {
        Ok(Ok(Some(line))) => {
            serde_json::from_str::<Reply>(&line).map_err(|e| format!("bad JSON from niri: {e}"))
        }
        Ok(Ok(None)) => Err("niri closed the connection".to_string()),
        Ok(Err(e)) => Err(format!("read failed: {e}")),
        Err(_) => Err("timed out waiting for niri response".to_string()),
    }
}

pub async fn send_request(
    request: Request,
) -> Result<Reply, Box<dyn std::error::Error + Send + Sync>> {
    let (reply_tx, reply_rx) = oneshot::channel();
    request_tx()
        .send((request, reply_tx))
        .map_err(|_| "niri request worker is not running")?;

    match reply_rx.await {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err("niri request worker dropped the reply".into()),
    }
}

pub async fn focus_window(id: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = Request::Action(Action::FocusWindow { id });
    match send_request(request).await? {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("failed to focus window: {e}").into()),
    }
}

pub async fn spawn(command: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = Request::Action(Action::Spawn {
        command: vec!["sh".to_string(), "-c".to_string(), command],
    });
    match send_request(request).await? {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("failed to spawn command: {e}").into()),
    }
}

pub async fn event_stream(
    tx: mpsc::UnboundedSender<Event>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = connect().await?;

    let mut request_str = serde_json::to_string(&Request::EventStream)?;
    request_str.push('\n');
    stream.write_all(request_str.as_bytes()).await?;

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    lines.next_line().await?;

    while let Some(line) = lines.next_line().await? {
        let event = match serde_json::from_str::<Event>(&line) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("Failed to parse event line: {e}");
                continue;
            }
        };

        if tx.send(event).is_err() {
            break;
        }
    }

    Ok(())
}
