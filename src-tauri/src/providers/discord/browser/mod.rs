pub(crate) mod chromium;
pub(crate) mod discovery;
pub(crate) mod firefox;
pub(crate) mod protocol;

use serde::Serialize;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::AppError;
use crate::providers::discord::session::{DiscordSessionOwner, DiscordSessionStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrowserFamily {
    ChromiumCdp,
    FirefoxBidi,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BrowserDescriptor {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) family: BrowserFamily,
    pub(crate) executable: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureProgress {
    Launching,
    WaitingForLogin,
    Verifying,
    Complete,
}

#[derive(Clone, Default)]
pub(crate) struct CaptureCancellation(Arc<AtomicBool>);

impl CaptureCancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub(crate) struct BrowserTokenCapture;

impl BrowserTokenCapture {
    pub(crate) fn discover() -> Vec<BrowserDescriptor> {
        discovery::discover()
    }

    pub(crate) async fn capture(
        browser: &BrowserDescriptor,
        expected_account_id: &str,
        remember: bool,
        session: &DiscordSessionOwner,
        cancellation: &CaptureCancellation,
        progress: impl Fn(CaptureProgress),
    ) -> Result<DiscordSessionStatus, AppError> {
        progress(CaptureProgress::Launching);
        let token = match browser.family {
            BrowserFamily::ChromiumCdp => {
                chromium::capture(&browser.executable, cancellation, &progress).await?
            }
            BrowserFamily::FirefoxBidi => {
                firefox::capture(&browser.executable, cancellation, &progress).await?
            }
        };
        progress(CaptureProgress::Verifying);
        let status = session
            .install_captured(expected_account_id, token, remember)
            .await?;
        progress(CaptureProgress::Complete);
        Ok(status)
    }
}

struct BrowserProcess {
    child: Child,
    _profile: tempfile::TempDir,
}

impl BrowserProcess {
    fn launch(
        executable: &std::path::Path,
        profile: tempfile::TempDir,
        arguments: &[std::ffi::OsString],
    ) -> Result<Self, AppError> {
        let child = Command::new(executable)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| capture_failed())?;
        Ok(Self {
            child,
            _profile: profile,
        })
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn capture_failed() -> AppError {
    AppError::InvalidRequest("Discord browser sign-in could not be completed".into())
}

fn capture_cancelled() -> AppError {
    AppError::InvalidRequest("Discord browser sign-in was cancelled".into())
}

#[cfg(test)]
mod tests;
