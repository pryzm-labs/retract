use crate::error::AppError;

#[cfg(target_os = "macos")]
pub async fn authenticate(reason: &str) -> Result<(), AppError> {
    if reason.trim().is_empty() {
        return Err(AppError::SystemAuthentication(
            "the authentication reason must not be empty".into(),
        ));
    }
    let reason = reason.to_owned();
    tokio::task::spawn_blocking(move || authenticate_blocking(&reason))
        .await
        .map_err(|_| {
            AppError::SystemAuthentication(
                "the macOS authentication worker ended unexpectedly".into(),
            )
        })?
}

#[cfg(target_os = "macos")]
fn authenticate_blocking(reason: &str) -> Result<(), AppError> {
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};
    // SAFETY: `new` and the two messages below are declared by LocalAuthentication.
    // The non-Send LAContext stays on this blocking worker for the full evaluation,
    // while the system reply block captures only a thread-safe channel sender.
    let context = unsafe { LAContext::new() };
    unsafe {
        context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication)
    }
    .map_err(|_| {
        AppError::SystemAuthentication(
            "macOS cannot verify the current device owner; check the login-password or Touch ID configuration".into(),
        )
    })?;

    let (sender, receiver) = mpsc::sync_channel(1);
    {
        let reply = RcBlock::new(move |success: Bool, _error: *mut NSError| {
            let _ = sender.try_send(success.as_bool());
        });
        let reason = NSString::from_str(reason);
        unsafe {
            context.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthentication,
                &reason,
                &reply,
            );
        }
    }

    match receiver.recv_timeout(Duration::from_secs(90)) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AppError::SystemAuthentication(
            "device-owner authentication was cancelled or rejected".into(),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(AppError::SystemAuthentication(
            "macOS ended the authentication request unexpectedly".into(),
        )),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            unsafe { context.invalidate() };
            Err(AppError::SystemAuthentication(
                "device-owner authentication timed out".into(),
            ))
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub async fn authenticate(_reason: &str) -> Result<(), AppError> {
    Err(AppError::SystemAuthentication(
        "high-impact actions are currently supported only on macOS".into(),
    ))
}
