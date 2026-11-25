//! Source discovery and workspace selection shared by offline archive providers.
use crate::{
    RuntimeState,
    compatibility::model_v2::{BootstrapRequest, BootstrapResponse, Empty},
    persistence::archive::ArchiveSourceEntry,
    provider_service::{decode, encode, safe, validate_version},
};
use retract_domain::{ErrorCode, SafeError, Scope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tauri::State;
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceView {
    scope: Scope,
    account_label: String,
    username: Option<String>,
    imported_at: Option<chrono::DateTime<chrono::Utc>>,
    warning_count: usize,
}

pub(super) fn source_view(entry: &ArchiveSourceEntry) -> SourceView {
    SourceView {
        scope: entry.source.scope(),
        account_label: entry.account.display_name.clone(),
        username: entry.account.username.clone(),
        imported_at: entry.source.imported_at,
        warning_count: entry.source.warnings.len(),
    }
}

pub(super) async fn ready_entries(
    runtime: &RuntimeState,
) -> Result<
    (
        Arc<crate::persistence::archive::ArchiveService>,
        Vec<ArchiveSourceEntry>,
    ),
    SafeError,
> {
    let archives = runtime.archives.open().await.map_err(archive_error)?;
    let entries = archives.ready_sources().await.map_err(archive_error)?;
    Ok((archives, entries))
}

fn archive_error(error: crate::persistence::archive::ArchiveError) -> SafeError {
    safe(match error {
        crate::persistence::archive::ArchiveError::UnavailableKey => {
            ErrorCode::AuthenticationRequired
        }
        crate::persistence::archive::ArchiveError::InvalidRecord
        | crate::persistence::archive::ArchiveError::InvalidStore => ErrorCode::InvalidArchive,
        crate::persistence::archive::ArchiveError::ScopeMismatch => ErrorCode::NotFound,
        _ => ErrorCode::Transient,
    })
}

#[tauri::command]
pub(crate) async fn list_sources_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<Empty> = decode(request)?;
    runtime
        .service
        .read()
        .await
        .check_optional(request.context.as_ref())?;
    let (_, entries) = ready_entries(&runtime).await?;
    encode(BootstrapResponse {
        contract_version: 2,
        context: runtime.service.read().await.context(),
        payload: entries.iter().map(source_view).collect::<Vec<_>>(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectArchive {
    scope: Scope,
}

#[tauri::command]
pub(crate) async fn select_archive_v2(
    runtime: State<'_, Arc<RuntimeState>>,
    request: Value,
) -> Result<Value, SafeError> {
    validate_version(&request)?;
    let request: BootstrapRequest<SelectArchive> = decode(request)?;
    let mut current = runtime.service.write().await;
    current.check_optional(request.context.as_ref())?;
    if current.has_workers().await {
        return Err(safe(ErrorCode::PermissionChanged));
    }
    current.cancel_searches();
    let (archives, entries) = ready_entries(&runtime).await?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.source.scope() == request.payload.scope)
        .ok_or_else(|| safe(ErrorCode::NotFound))?;
    // Prepare and validate the candidate before retiring the usable workspace.
    // Construction/recovery can fail on retained state; failure must not strand
    // the user behind a stopped service or prevent a subsequent source retry.
    let connection = super::archive::connect(&runtime, entry, archives)?;
    let candidate = crate::provider_service::ProviderService::new(connection);
    let snapshot = match candidate
        .bootstrap(
            serde_json::json!({"contractVersion":2,"context":candidate.context(),"payload":{}}),
        )
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            candidate.shutdown().await;
            return Err(error);
        }
    };
    current.shutdown().await;
    *current = candidate;
    Ok(snapshot)
}
