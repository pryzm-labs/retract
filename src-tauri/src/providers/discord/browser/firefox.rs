use super::protocol::ProtocolParser;
use super::{
    BrowserProcess, CaptureCancellation, CaptureProgress, capture_cancelled, capture_failed,
};
use futures_util::{SinkExt, StreamExt};
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use zeroize::Zeroizing;

use crate::error::AppError;

const LOGIN_URL: &str = "https://discord.com/app";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

pub(super) fn launch_arguments(profile: &Path, port: u16) -> Vec<OsString> {
    vec![
        OsString::from("--profile"),
        profile.as_os_str().to_owned(),
        OsString::from("--remote-debugging-port"),
        OsString::from(port.to_string()),
        OsString::from("--new-instance"),
        OsString::from(LOGIN_URL),
    ]
}

pub(super) async fn capture(
    executable: &Path,
    cancellation: &CaptureCancellation,
    progress: &impl Fn(CaptureProgress),
) -> Result<Zeroizing<String>, AppError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| capture_failed())?;
    let port = listener.local_addr().map_err(|_| capture_failed())?.port();
    drop(listener);
    let profile = tempfile::Builder::new()
        .prefix("retract-discord-firefox-")
        .tempdir()
        .map_err(|_| capture_failed())?;
    let arguments = launch_arguments(profile.path(), port);
    let _process = BrowserProcess::launch(executable, profile, &arguments)?;
    let endpoint = format!("ws://127.0.0.1:{port}/session");
    let mut socket = connect_when_ready(&endpoint, cancellation).await?;
    socket
        .send(Message::Text(
            r#"{"id":1,"method":"session.new","params":{"capabilities":{"alwaysMatch":{"acceptInsecureCerts":false}}}}"#.into(),
        ))
        .await
        .map_err(|_| capture_failed())?;
    wait_for_success(&mut socket, 1, cancellation).await?;
    socket
        .send(Message::Text(
            r#"{"id":2,"method":"session.subscribe","params":{"events":["network.beforeRequestSent"]}}"#.into(),
        ))
        .await
        .map_err(|_| capture_failed())?;
    wait_for_success(&mut socket, 2, cancellation).await?;
    progress(CaptureProgress::WaitingForLogin);

    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let mut parser = ProtocolParser::default();
    loop {
        if cancellation.is_cancelled() {
            return Err(capture_cancelled());
        }
        if Instant::now() >= deadline {
            return Err(capture_failed());
        }
        let message = match tokio::time::timeout(Duration::from_millis(250), socket.next()).await {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(_)) | None) => return Err(capture_failed()),
            Err(_) => continue,
        };
        let Message::Text(text) = message else {
            continue;
        };
        let event = Zeroizing::new(text.to_string());
        if let Ok(Some(captured)) = parser.parse_bidi(&event) {
            return Ok(captured.into_inner());
        }
    }
}

async fn connect_when_ready(
    endpoint: &str,
    cancellation: &CaptureCancellation,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    AppError,
> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if cancellation.is_cancelled() {
            return Err(capture_cancelled());
        }
        if let Ok((socket, _)) = tokio_tungstenite::connect_async(endpoint).await {
            return Ok(socket);
        }
        if Instant::now() >= deadline {
            return Err(capture_failed());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_success<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    expected_id: u64,
    cancellation: &CaptureCancellation,
) -> Result<(), AppError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if cancellation.is_cancelled() {
            return Err(capture_cancelled());
        }
        if Instant::now() >= deadline {
            return Err(capture_failed());
        }
        let message = match tokio::time::timeout(Duration::from_millis(250), socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(_))) | Err(_) => continue,
            Ok(Some(Err(_)) | None) => return Err(capture_failed()),
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&message) else {
            continue;
        };
        if value.get("id").and_then(serde_json::Value::as_u64) == Some(expected_id) {
            return if value.get("result").is_some() {
                Ok(())
            } else {
                Err(capture_failed())
            };
        }
    }
}
