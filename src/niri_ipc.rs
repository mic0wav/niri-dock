use niri_ipc_types::{Action, Event, Reply, Request, Response};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};

pub const APP_ID: &str = "org.niri.dock";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NIRI_SOCKET is not set — is this running inside a niri session?")]
    SocketNotSet,

    #[error("failed to connect to the niri socket: {0}")]
    Connect(#[source] std::io::Error),

    #[error("failed to serialize request: {0}")]
    Serialize(#[source] serde_json::Error),

    #[error("failed to write to the niri socket: {0}")]
    Write(#[source] std::io::Error),

    #[error("failed to read from the niri socket: {0}")]
    Read(#[source] std::io::Error),

    #[error("niri closed the connection")]
    ConnectionClosed,

    #[error("timed out waiting for a reply from niri")]
    Timeout,

    #[error("failed to parse niri's reply: {0}")]
    Deserialize(#[source] serde_json::Error),

    #[error("niri reported an error: {0}")]
    Niri(String),

    #[error("the niri request worker is not running")]
    WorkerGone,

    #[error("the niri request worker dropped the reply before answering")]
    ReplyDropped,
}

pub type Result<T> = std::result::Result<T, Error>;

pub async fn connect() -> Result<UnixStream> {
    let socket_path = get_niri_socket()?;
    UnixStream::connect(&socket_path)
        .await
        .map_err(Error::Connect)
}

fn get_niri_socket() -> Result<PathBuf> {
    std::env::var("NIRI_SOCKET")
        .map(PathBuf::from)
        .map_err(|_| Error::SocketNotSet)
}

type ReplyTx = oneshot::Sender<Result<Reply>>;

fn request_tx() -> &'static mpsc::UnboundedSender<(Request, ReplyTx)> {
    static REQUEST_TX: OnceLock<mpsc::UnboundedSender<(Request, ReplyTx)>> = OnceLock::new();
    REQUEST_TX.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        crate::runtime().spawn(request_worker(rx));
        tx
    })
}

async fn request_worker(mut rx: mpsc::UnboundedReceiver<(Request, ReplyTx)>) {
    let mut backoff = crate::backoff::Backoff::niri_default();

    loop {
        let started = std::time::Instant::now();
        let stream = match connect().await {
            Ok(s) => s,
            Err(e) => {
                log::error!("REQ: connect failed: {e}");
                tokio::time::sleep(backoff.advance(started.elapsed())).await;
                continue;
            }
        };

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

        tokio::time::sleep(backoff.advance(started.elapsed())).await;
    }
}

async fn send_on_connection(
    writer: &mut OwnedWriteHalf,
    lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
    request: Request,
) -> Result<Reply> {
    let mut request_str = serde_json::to_string(&request).map_err(Error::Serialize)?;
    request_str.push('\n');

    writer
        .write_all(request_str.as_bytes())
        .await
        .map_err(Error::Write)?;

    match tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await {
        Ok(Ok(Some(line))) => serde_json::from_str::<Reply>(&line).map_err(Error::Deserialize),
        Ok(Ok(None)) => Err(Error::ConnectionClosed),
        Ok(Err(e)) => Err(Error::Read(e)),
        Err(_) => Err(Error::Timeout),
    }
}

pub async fn send_request(request: Request) -> Result<Response> {
    let (reply_tx, reply_rx) = oneshot::channel();
    request_tx()
        .send((request, reply_tx))
        .map_err(|_| Error::WorkerGone)?;

    match reply_rx.await {
        Ok(Ok(reply)) => reply.map_err(Error::Niri),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(Error::ReplyDropped),
    }
}

pub async fn focus_window(id: u64) -> Result<()> {
    send_request(Request::Action(Action::FocusWindow { id })).await?;
    Ok(())
}

pub async fn spawn(command: String) -> Result<()> {
    send_request(Request::Action(Action::Spawn {
        command: vec!["sh".to_string(), "-c".to_string(), command],
    }))
    .await?;
    Ok(())
}

pub async fn watch_events(tx: mpsc::UnboundedSender<Event>) {
    let mut backoff = crate::backoff::Backoff::niri_default();
    loop {
        let started = std::time::Instant::now();
        match event_stream(tx.clone()).await {
            Ok(()) => log::warn!("Niri event stream closed cleanly"),
            Err(e) => log::error!("Niri event stream ended: {e}"),
        }

        let delay = backoff.advance(started.elapsed());
        log::info!("Reconnecting to niri in {delay:?}");
        tokio::time::sleep(delay).await;
    }
}

pub async fn event_stream(tx: mpsc::UnboundedSender<Event>) -> Result<()> {
    let mut stream = connect().await?;

    let mut request_str = serde_json::to_string(&Request::EventStream).map_err(Error::Serialize)?;
    request_str.push('\n');
    stream
        .write_all(request_str.as_bytes())
        .await
        .map_err(Error::Write)?;

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    lines.next_line().await.map_err(Error::Read)?;

    while let Some(line) = lines.next_line().await.map_err(Error::Read)? {
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
