use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_double, c_void},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::Value;
use tokio::sync::{broadcast, oneshot};
use uuid::Uuid;

use crate::error::AppError;

#[cfg(target_os = "macos")]
type CreateFn = unsafe extern "C" fn() -> *mut c_void;
type SendFn = unsafe extern "C" fn(*mut c_void, *const c_char);
type ReceiveFn = unsafe extern "C" fn(*mut c_void, c_double) -> *const c_char;
type DestroyFn = unsafe extern "C" fn(*mut c_void);
#[cfg(target_os = "macos")]
type SetLogVerbosityFn = unsafe extern "C" fn(i32);

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn td_json_client_create() -> *mut c_void;
    fn td_json_client_send(client: *mut c_void, request: *const c_char);
    fn td_json_client_receive(client: *mut c_void, timeout: c_double) -> *const c_char;
    fn td_json_client_destroy(client: *mut c_void);
    fn td_set_log_verbosity_level(level: i32);
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const DEFAULT_TDLIB_LOG_VERBOSITY: i32 = 0;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const RECEIVE_TIMEOUT_SECONDS: c_double = 1.0;

/// A narrow, owned wrapper around TDLib's legacy per-client JSON C interface.
///
/// TDLib documents `send` as thread-safe and requires exactly one concurrent
/// receiver. Retract enforces that contract with one dedicated receiver thread.
#[derive(Clone)]
pub struct TdJsonClient {
    backend: ClientBackend,
}

#[derive(Clone)]
enum ClientBackend {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Native(Arc<Inner>),
    #[cfg(test)]
    Scripted(Arc<ScriptedState>),
}

#[cfg(test)]
struct ScriptedState {
    exchanges: Mutex<Vec<ScriptedExchange>>,
    traces: Mutex<Vec<ScriptedRequestTrace>>,
    updates: broadcast::Sender<Value>,
}

#[cfg(test)]
struct ScriptedExchange {
    request: Value,
    response: ScriptedResponse,
}

#[cfg(test)]
enum ScriptedResponse {
    Immediate(Value),
    Delayed(oneshot::Receiver<Value>),
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScriptedRequestTrace {
    pub kind: String,
    pub chat_id: Option<i64>,
    pub message_id: Option<i64>,
    pub message_ids: Vec<i64>,
    pub from_message_id: Option<i64>,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ScriptedTdJson {
    state: Arc<ScriptedState>,
}

#[cfg(test)]
pub(crate) struct ScriptedDelayedResponse {
    sender: Option<oneshot::Sender<Value>>,
}

struct Inner {
    handle: *mut c_void,
    send: SendFn,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    receive: ReceiveFn,
    destroy: DestroyFn,
    closing: AtomicBool,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    updates: broadcast::Sender<Value>,
}

struct PendingRequest {
    inner: Arc<Inner>,
    extra: String,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.inner.pending.lock() {
            pending.remove(&self.extra);
        }
    }
}

// Safety: the opaque TDLib client is only received from on the dedicated
// thread. TDLib explicitly permits sending requests from any thread.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl TdJsonClient {
    pub fn load() -> Result<Self, AppError> {
        #[cfg(not(target_os = "macos"))]
        {
            Err(AppError::Gateway("TDLIB_PLATFORM_UNSUPPORTED".into()))
        }
        #[cfg(target_os = "macos")]
        {
            Self::load_macos()
        }
    }

    #[cfg(target_os = "macos")]
    fn load_macos() -> Result<Self, AppError> {
        // TDLib is verified in build.rs and statically linked into Retract. No
        // renderer-controlled or persisted path can select executable code.
        let (create, send, receive, destroy, set_log_verbosity) = (
            td_json_client_create as CreateFn,
            td_json_client_send as SendFn,
            td_json_client_receive as ReceiveFn,
            td_json_client_destroy as DestroyFn,
            td_set_log_verbosity_level as SetLogVerbosityFn,
        );
        let verbosity =
            tdlib_log_verbosity(std::env::var("RETRACT_TDLIB_LOG_VERBOSITY").ok().as_deref());
        // Safety: the symbol was loaded with TDLib's documented C signature.
        // Set the process-wide level before creating a client so even its startup
        // and first receive calls cannot flood the app console.
        unsafe { set_log_verbosity(verbosity) };
        // Safety: `create` has the verified TDLib signature and needs no args.
        let handle = unsafe { create() };
        if handle.is_null() {
            return Err(AppError::Gateway("TDLIB_CLIENT_CREATE_FAILED".into()));
        }
        let (updates, _) = broadcast::channel(512);
        let inner = Arc::new(Inner {
            handle,
            send,
            receive,
            destroy,
            closing: AtomicBool::new(false),
            pending: Mutex::new(HashMap::new()),
            updates,
        });
        spawn_receiver(Arc::downgrade(&inner));
        Ok(Self {
            backend: ClientBackend::Native(inner),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        match &self.backend {
            ClientBackend::Native(inner) => inner.updates.subscribe(),
            #[cfg(test)]
            ClientBackend::Scripted(state) => state.updates.subscribe(),
        }
    }

    pub async fn request(&self, request: Value) -> Result<Value, AppError> {
        match &self.backend {
            ClientBackend::Native(inner) => native_request(Arc::clone(inner), request).await,
            #[cfg(test)]
            ClientBackend::Scripted(state) => scripted_request(state, request).await,
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted() -> (Self, ScriptedTdJson) {
        let (updates, _) = broadcast::channel(512);
        let state = Arc::new(ScriptedState {
            exchanges: Mutex::new(Vec::new()),
            traces: Mutex::new(Vec::new()),
            updates,
        });
        (
            Self {
                backend: ClientBackend::Scripted(Arc::clone(&state)),
            },
            ScriptedTdJson { state },
        )
    }
}

async fn native_request(inner: Arc<Inner>, mut request: Value) -> Result<Value, AppError> {
    if inner.closing.load(Ordering::Acquire) {
        return Err(AppError::Gateway("TDLIB_CLIENT_CLOSED".into()));
    }
    let extra = Uuid::new_v4().to_string();
    request
        .as_object_mut()
        .ok_or_else(|| AppError::InvalidRequest("TDLib request must be an object".into()))?
        .insert("@extra".into(), Value::String(extra.clone()));
    let serialized = serde_json::to_string(&request)
        .map_err(|error| AppError::Gateway(format!("TDLIB_JSON_ENCODE: {error}")))?;
    let c_request = CString::new(serialized)
        .map_err(|_| AppError::Gateway("TDLIB_JSON_CONTAINS_NUL".into()))?;
    let (sender, receiver) = oneshot::channel();
    inner
        .pending
        .lock()
        .map_err(|_| AppError::StateUnavailable)?
        .insert(extra.clone(), sender);
    // Keep pending request cleanup cancellation-safe. Foreground operations
    // use a shorter aggregate timeout than the generic TDLib request limit,
    // so dropping this future must not strand a sender in the routing map.
    let _pending_request = PendingRequest {
        inner: Arc::clone(&inner),
        extra: extra.clone(),
    };
    // Safety: the handle and function pointer live in the same Inner. TDLib
    // copies the null-terminated request before this function returns.
    unsafe { (inner.send)(inner.handle, c_request.as_ptr()) };

    let response = tokio::time::timeout(Duration::from_secs(45), receiver)
        .await
        .map_err(|_| AppError::Gateway("TDLIB_REQUEST_TIMEOUT".into()))?
        .map_err(|_| AppError::Gateway("TDLIB_RESPONSE_CHANNEL_CLOSED".into()))?;
    checked_response(response)
}

fn checked_response(response: Value) -> Result<Value, AppError> {
    if response.get("@type").and_then(Value::as_str) == Some("error") {
        let code = value_i64(response.get("code")).unwrap_or_default();
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        return Err(AppError::Gateway(format!("{code} {message}")));
    }
    Ok(response)
}

#[cfg(test)]
async fn scripted_request(state: &ScriptedState, request: Value) -> Result<Value, AppError> {
    let trace = scripted_trace(&request);
    state
        .traces
        .lock()
        .map_err(|_| AppError::StateUnavailable)?
        .push(trace.clone());
    let response = {
        let mut exchanges = state
            .exchanges
            .lock()
            .map_err(|_| AppError::StateUnavailable)?;
        let Some(index) = exchanges
            .iter()
            .position(|exchange| exchange.request == request)
        else {
            return Err(AppError::Gateway(format!(
                "SCRIPTED_TDJSON_UNEXPECTED_REQUEST:{}",
                trace.kind
            )));
        };
        exchanges.remove(index).response
    };
    let response = match response {
        ScriptedResponse::Immediate(response) => response,
        ScriptedResponse::Delayed(receiver) => receiver
            .await
            .map_err(|_| AppError::Gateway("SCRIPTED_TDJSON_RESPONSE_DROPPED".into()))?,
    };
    checked_response(response)
}

#[cfg(test)]
fn scripted_trace(request: &Value) -> ScriptedRequestTrace {
    ScriptedRequestTrace {
        kind: request
            .get("@type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        chat_id: value_i64(request.get("chat_id")),
        message_id: value_i64(request.get("message_id")),
        message_ids: request
            .get("message_ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value_i64(Some(value)))
            .collect(),
        from_message_id: value_i64(request.get("from_message_id")),
    }
}

#[cfg(test)]
impl ScriptedTdJson {
    pub(crate) fn respond(&self, request: Value, response: Value) {
        self.state
            .exchanges
            .lock()
            .expect("scripted TDJSON exchange lock")
            .push(ScriptedExchange {
                request,
                response: ScriptedResponse::Immediate(response),
            });
    }

    pub(crate) fn delay_response(&self, request: Value) -> ScriptedDelayedResponse {
        let (sender, receiver) = oneshot::channel();
        self.state
            .exchanges
            .lock()
            .expect("scripted TDJSON exchange lock")
            .push(ScriptedExchange {
                request,
                response: ScriptedResponse::Delayed(receiver),
            });
        ScriptedDelayedResponse {
            sender: Some(sender),
        }
    }

    pub(crate) fn emit_update(&self, update: Value) {
        let _ = self.state.updates.send(update);
    }

    pub(crate) fn traces(&self) -> Vec<ScriptedRequestTrace> {
        self.state
            .traces
            .lock()
            .expect("scripted TDJSON trace lock")
            .clone()
    }

    pub(crate) fn assert_drained(&self) {
        let exchanges = self
            .state
            .exchanges
            .lock()
            .expect("scripted TDJSON exchange lock");
        assert!(
            exchanges.is_empty(),
            "{} scripted TDJSON exchanges were not consumed",
            exchanges.len()
        );
    }
}

#[cfg(test)]
impl ScriptedDelayedResponse {
    pub(crate) fn respond(mut self, response: Value) {
        self.sender
            .take()
            .expect("scripted delayed response is single use")
            .send(response)
            .expect("scripted delayed request is still pending");
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn tdlib_log_verbosity(raw: Option<&str>) -> i32 {
    raw.and_then(|value| value.parse::<i32>().ok())
        .filter(|value| (0..=5).contains(value))
        .unwrap_or(DEFAULT_TDLIB_LOG_VERBOSITY)
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        // Safety: the last strong Inner reference can only be dropped between
        // receive calls; the receiver holds a strong reference while inside C.
        unsafe { (self.destroy)(self.handle) };
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn spawn_receiver(inner: Weak<Inner>) {
    thread::Builder::new()
        .name("retract-tdlib-receive".into())
        .spawn(move || {
            loop {
                let Some(inner) = inner.upgrade() else {
                    break;
                };
                if inner.closing.load(Ordering::Acquire) {
                    break;
                }
                // Safety: this is the only thread that invokes receive. The result
                // is copied before the next receive call, as TDLib requires.
                let pointer = unsafe { (inner.receive)(inner.handle, RECEIVE_TIMEOUT_SECONDS) };
                if pointer.is_null() {
                    continue;
                }
                // Safety: TDLib returns a valid, null-terminated string which stays
                // alive until the next receive call on this client.
                let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
                let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
                    continue;
                };
                route_received(&inner, value);
            }
        })
        .expect("failed to start TDLib receive thread");
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn route_received(inner: &Inner, value: Value) {
    let extra = value
        .get("@extra")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(extra) = extra
        && let Ok(mut pending) = inner.pending.lock()
        && let Some(sender) = pending.remove(&extra)
    {
        let _ = sender.send(value);
        return;
    }
    let _ = inner.updates.send(value);
}

fn value_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tdlib_integer_encodings() {
        assert_eq!(value_i64(Some(&Value::from(42))), Some(42));
        assert_eq!(
            value_i64(Some(&Value::from("9000000000"))),
            Some(9_000_000_000)
        );
        assert_eq!(value_i64(Some(&Value::Null)), None);
    }

    #[test]
    fn keeps_tdlib_quiet_unless_a_valid_developer_override_is_set() {
        assert_eq!(tdlib_log_verbosity(None), 0);
        assert_eq!(tdlib_log_verbosity(Some("4")), 4);
        assert_eq!(tdlib_log_verbosity(Some("-1")), 0);
        assert_eq!(tdlib_log_verbosity(Some("6")), 0);
        assert_eq!(tdlib_log_verbosity(Some("verbose")), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn statically_linked_tdlib_and_sqlcipher_use_the_reviewed_native_stack() {
        let client = TdJsonClient::load().expect("statically linked TDLib must initialize");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let response = runtime
            .block_on(client.request(serde_json::json!({
                "@type": "getOption",
                "name": "version"
            })))
            .expect("TDLib must answer getOption(version)");
        assert_eq!(
            response.get("value").and_then(Value::as_str),
            Some("1.8.64")
        );

        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("native-stack-smoke.sqlite3");
        let key = format!("x'{}'", "31".repeat(32));
        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection.pragma_update(None, "key", &key).unwrap();
            let provider: String = connection
                .pragma_query_value(None, "cipher_provider", |row| row.get(0))
                .unwrap();
            let provider_version: String = connection
                .pragma_query_value(None, "cipher_provider_version", |row| row.get(0))
                .unwrap();
            assert_eq!(provider, "openssl");
            assert_eq!(provider_version, "OpenSSL 3.6.3 9 Jun 2026");
            connection
                .execute_batch(
                    "CREATE TABLE native_smoke(value TEXT NOT NULL);\
                     INSERT INTO native_smoke(value) VALUES ('verified');",
                )
                .unwrap();
        }
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection.pragma_update(None, "key", &key).unwrap();
        let value: String = connection
            .query_row("SELECT value FROM native_smoke", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "verified");
    }
}
