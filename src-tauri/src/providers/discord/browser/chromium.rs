use super::protocol::ProtocolParser;
use super::{
    BrowserProcess, CaptureCancellation, CaptureProgress, capture_cancelled, capture_failed,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use zeroize::{Zeroize, Zeroizing};

use crate::error::AppError;

const LOGIN_URL: &str = "https://discord.com/app";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

pub(super) fn launch_arguments(profile: &Path) -> Vec<OsString> {
    vec![
        OsString::from(format!("--user-data-dir={}", profile.display())),
        OsString::from("--remote-debugging-address=127.0.0.1"),
        OsString::from("--remote-debugging-port=0"),
        OsString::from("--no-first-run"),
        OsString::from("--no-default-browser-check"),
        OsString::from(LOGIN_URL),
    ]
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DebugTarget {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    web_socket_debugger_url: String,
}

pub(super) async fn capture(
    executable: &Path,
    cancellation: &CaptureCancellation,
    progress: &impl Fn(CaptureProgress),
) -> Result<Zeroizing<String>, AppError> {
    let profile = tempfile::Builder::new()
        .prefix("retract-discord-chromium-")
        .tempdir()
        .map_err(|_| capture_failed())?;
    let active_port = profile.path().join("DevToolsActivePort");
    let arguments = launch_arguments(profile.path());
    let _process = BrowserProcess::launch(executable, profile, &arguments)?;
    let (port, browser_path) = wait_for_active_port(&active_port, cancellation).await?;
    let debugger_url = wait_for_target(port, &browser_path, cancellation).await?;
    let (mut socket, _) = tokio_tungstenite::connect_async(debugger_url.as_str())
        .await
        .map_err(|_| capture_failed())?;
    socket
        .send(Message::Text(
            r#"{"id":1,"method":"Network.enable","params":{}}"#.into(),
        ))
        .await
        .map_err(|_| capture_failed())?;
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
        let mut event = Zeroizing::new(text.to_string());
        if let Ok(Some(captured)) = parser.parse_cdp(&event) {
            event.zeroize();
            return Ok(captured.into_inner());
        }
    }
}

async fn wait_for_active_port(
    path: &Path,
    cancellation: &CaptureCancellation,
) -> Result<(u16, String), AppError> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if cancellation.is_cancelled() {
            return Err(capture_cancelled());
        }
        if let Ok(mut value) = std::fs::read_to_string(path) {
            let mut lines = value.lines();
            let parsed = lines.next().and_then(|port| port.parse::<u16>().ok());
            let browser_path = lines.next().filter(|path| path.starts_with('/'));
            if let (Some(port), Some(browser_path)) = (parsed, browser_path) {
                let result = (port, browser_path.to_owned());
                value.zeroize();
                return Ok(result);
            }
            value.zeroize();
        }
        if Instant::now() >= deadline {
            return Err(capture_failed());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_target(
    port: u16,
    _browser_path: &str,
    cancellation: &CaptureCancellation,
) -> Result<url::Url, AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|_| capture_failed())?;
    let endpoint = format!("http://127.0.0.1:{port}/json/list");
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if cancellation.is_cancelled() {
            return Err(capture_cancelled());
        }
        if let Some(url) = fetch_target(&client, &endpoint, port).await {
            return Ok(url);
        }
        if Instant::now() >= deadline {
            return Err(capture_failed());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn fetch_target(client: &reqwest::Client, endpoint: &str, port: u16) -> Option<url::Url> {
    let response = client.get(endpoint).send().await.ok()?;
    let bytes = response.bytes().await.ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let targets = serde_json::from_slice::<Vec<DebugTarget>>(&bytes).ok()?;
    let target = targets
        .into_iter()
        .find(|target| target.kind == "page" && target.url.starts_with("https://discord.com/"))?;
    let url = url::Url::parse(&target.web_socket_debugger_url).ok()?;
    (url.scheme() == "ws" && url.host_str() == Some("127.0.0.1") && url.port() == Some(port))
        .then_some(url)
}
