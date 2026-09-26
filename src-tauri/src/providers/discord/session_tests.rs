use super::session::{
    DiscordCredentialStore, DiscordIdentityClient, DiscordSessionOwner, DiscordSessionState,
    StoredDiscordCredential, VerifiedDiscordIdentity,
};
use async_trait::async_trait;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;
use zeroize::Zeroizing;

const OWNER: &str = "9007199254741001";
const OTHER: &str = "9007199254741002";
const TOKEN: &str = "synthetic.discord.token-value_123456789";

#[derive(Default)]
struct MemoryStore {
    credential: Mutex<Option<(String, String)>>,
    loads: AtomicUsize,
}

impl DiscordCredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<StoredDiscordCredential>, crate::error::AppError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .credential
            .lock()
            .unwrap()
            .as_ref()
            .map(|(account_id, token)| {
                StoredDiscordCredential::new(account_id.clone(), Zeroizing::new(token.clone()))
                    .unwrap()
            }))
    }

    fn save(&self, credential: &StoredDiscordCredential) -> Result<(), crate::error::AppError> {
        self.credential.lock().unwrap().replace((
            credential.account_id().to_owned(),
            credential.expose_token_for_request(|value| value.to_owned()),
        ));
        Ok(())
    }

    fn forget(&self) -> Result<(), crate::error::AppError> {
        self.credential.lock().unwrap().take();
        Ok(())
    }
}

struct IdentityClient {
    identity: VerifiedDiscordIdentity,
}

struct DelayedIdentityClient {
    identity: VerifiedDiscordIdentity,
    started: Notify,
    release: Notify,
}

struct BlockingLoadStore {
    credential: Mutex<Option<(String, String)>>,
    loads: AtomicUsize,
    started: Notify,
    release: (Mutex<bool>, Condvar),
}

struct ForgetBarrierStore {
    credential: Mutex<Option<(String, String)>>,
    forget_started: Notify,
    forget_release: (Mutex<bool>, Condvar),
}

impl ForgetBarrierStore {
    fn new() -> Self {
        Self {
            credential: Mutex::new(Some((OWNER.into(), TOKEN.into()))),
            forget_started: Notify::new(),
            forget_release: (Mutex::new(false), Condvar::new()),
        }
    }

    fn release_forget(&self) {
        let (released, wake) = &self.forget_release;
        *released.lock().unwrap() = true;
        wake.notify_all();
    }
}

impl DiscordCredentialStore for ForgetBarrierStore {
    fn load(&self) -> Result<Option<StoredDiscordCredential>, crate::error::AppError> {
        Ok(self
            .credential
            .lock()
            .unwrap()
            .as_ref()
            .map(|(account_id, token)| {
                StoredDiscordCredential::new(account_id.clone(), Zeroizing::new(token.clone()))
                    .unwrap()
            }))
    }

    fn save(&self, credential: &StoredDiscordCredential) -> Result<(), crate::error::AppError> {
        self.credential.lock().unwrap().replace((
            credential.account_id().to_owned(),
            credential.expose_token_for_request(str::to_owned),
        ));
        Ok(())
    }

    fn forget(&self) -> Result<(), crate::error::AppError> {
        self.forget_started.notify_one();
        let (released, wake) = &self.forget_release;
        let mut released = released.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
        self.credential.lock().unwrap().take();
        Ok(())
    }
}

struct RegistrationBarrier {
    registered: Notify,
    release: (Mutex<bool>, Condvar),
}

impl RegistrationBarrier {
    fn new() -> Self {
        Self {
            registered: Notify::new(),
            release: (Mutex::new(false), Condvar::new()),
        }
    }

    fn wait(&self) {
        self.registered.notify_one();
        let (released, wake) = &self.release;
        let mut released = released.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
    }

    fn release(&self) {
        let (released, wake) = &self.release;
        *released.lock().unwrap() = true;
        wake.notify_all();
    }
}

impl BlockingLoadStore {
    fn new() -> Self {
        Self {
            credential: Mutex::new(Some((OWNER.into(), TOKEN.into()))),
            loads: AtomicUsize::new(0),
            started: Notify::new(),
            release: (Mutex::new(false), Condvar::new()),
        }
    }

    fn release_load(&self) {
        let (released, wake) = &self.release;
        *released.lock().unwrap() = true;
        wake.notify_all();
    }
}

impl DiscordCredentialStore for BlockingLoadStore {
    fn load(&self) -> Result<Option<StoredDiscordCredential>, crate::error::AppError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let result = self
            .credential
            .lock()
            .unwrap()
            .as_ref()
            .map(|(account_id, token)| {
                StoredDiscordCredential::new(account_id.clone(), Zeroizing::new(token.clone()))
                    .unwrap()
            });
        self.started.notify_one();
        let (released, wake) = &self.release;
        let mut released = released.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
        Ok(result)
    }

    fn save(&self, credential: &StoredDiscordCredential) -> Result<(), crate::error::AppError> {
        self.credential.lock().unwrap().replace((
            credential.account_id().to_owned(),
            credential.expose_token_for_request(str::to_owned),
        ));
        Ok(())
    }

    fn forget(&self) -> Result<(), crate::error::AppError> {
        self.credential.lock().unwrap().take();
        Ok(())
    }
}

#[async_trait]
impl DiscordIdentityClient for DelayedIdentityClient {
    async fn verify(&self, token: &str) -> Result<VerifiedDiscordIdentity, crate::error::AppError> {
        assert_eq!(token, TOKEN);
        self.started.notify_one();
        self.release.notified().await;
        Ok(self.identity.clone())
    }
}

#[async_trait]
impl DiscordIdentityClient for IdentityClient {
    async fn verify(&self, token: &str) -> Result<VerifiedDiscordIdentity, crate::error::AppError> {
        assert_eq!(token, TOKEN);
        Ok(self.identity.clone())
    }
}

fn owner(client_id: &str) -> (DiscordSessionOwner, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let client = Arc::new(IdentityClient {
        identity: VerifiedDiscordIdentity {
            account_id: client_id.to_owned(),
            username: "synthetic_user".into(),
            display_name: "Synthetic User".into(),
        },
    });
    (
        DiscordSessionOwner::with_dependencies(client, store.clone()),
        store,
    )
}

#[test]
fn manual_token_validation_rejects_unsafe_values_without_calling_identity() {
    for token in [
        "",
        "   ",
        "Bot abcdefghijklmnop",
        "Bearer abcdefghijklmnop",
        "bad token",
        "bad\nvalue",
    ] {
        assert!(StoredDiscordCredential::new(OWNER.into(), Zeroizing::new(token.into())).is_err());
    }
    assert!(StoredDiscordCredential::new("01".into(), Zeroizing::new(TOKEN.into())).is_err());
    assert!(StoredDiscordCredential::new(OWNER.into(), Zeroizing::new("x".repeat(1025))).is_err());
}

#[test]
fn manual_submission_matches_archive_owner_and_never_echoes_token() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (owner, store) = owner(OWNER);
    let status = runtime
        .block_on(owner.submit_manual(OWNER, TOKEN, false))
        .unwrap();
    assert_eq!(status.state, DiscordSessionState::Ready);
    assert_eq!(status.account_id.as_deref(), Some(OWNER));
    assert!(!status.remembered);
    assert!(store.credential.lock().unwrap().is_none());
    assert!(!format!("{status:?}").contains(TOKEN));
    let serialized = serde_json::to_string(&status).unwrap();
    assert!(!serialized.contains(TOKEN));
    assert_eq!(
        owner.with_token(OWNER, |token| token.to_owned()).unwrap(),
        TOKEN
    );
}

#[test]
fn mismatched_identity_is_rejected_and_remember_is_opt_in() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (mismatch, mismatch_store) = owner(OTHER);
    assert!(
        runtime
            .block_on(mismatch.submit_manual(OWNER, TOKEN, true))
            .is_err()
    );
    assert_eq!(mismatch.status().state, DiscordSessionState::Disconnected);
    assert!(mismatch_store.credential.lock().unwrap().is_none());

    let (matching, matching_store) = owner(OWNER);
    let status = runtime
        .block_on(matching.submit_manual(OWNER, TOKEN, true))
        .unwrap();
    assert!(status.remembered);
    assert_eq!(
        matching_store
            .credential
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .0,
        OWNER
    );
    matching.forget().unwrap();
    assert_eq!(matching.status().state, DiscordSessionState::Disconnected);
    assert!(matching_store.credential.lock().unwrap().is_none());
}

#[test]
fn remembered_session_is_loaded_only_once_for_the_selected_account() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (owner, store) = owner(OWNER);
    store
        .credential
        .lock()
        .unwrap()
        .replace((OWNER.into(), TOKEN.into()));

    let first = runtime.block_on(owner.load_remembered(OWNER)).unwrap();
    let second = runtime.block_on(owner.load_remembered(OWNER)).unwrap();

    assert_eq!(first.state, DiscordSessionState::Ready);
    assert_eq!(second.state, DiscordSessionState::Ready);
    assert!(first.remembered);
    assert_eq!(store.loads.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_automatic_restore_does_not_reprompt_keychain_for_the_same_account() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (owner, store) = owner(OWNER);
    store
        .credential
        .lock()
        .unwrap()
        .replace((OWNER.into(), TOKEN.into()));

    assert!(runtime.block_on(owner.load_remembered(OTHER)).is_err());
    let second = runtime.block_on(owner.load_remembered(OTHER)).unwrap();

    assert_eq!(second.state, DiscordSessionState::Disconnected);
    assert_eq!(store.loads.load(Ordering::SeqCst), 1);
}

#[test]
fn switching_archives_never_exposes_the_previous_account_as_ready() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (owner, _) = owner(OWNER);
    runtime
        .block_on(owner.submit_manual(OWNER, TOKEN, true))
        .unwrap();

    assert!(runtime.block_on(owner.load_remembered(OTHER)).is_err());
    let repeated = runtime.block_on(owner.load_remembered(OTHER)).unwrap();

    assert_eq!(repeated.state, DiscordSessionState::Disconnected);
    assert!(owner.with_token(OWNER, |_| ()).is_err());
}

#[test]
fn shutdown_drops_live_authority_and_wrong_scope_never_receives_token() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (owner, _) = owner(OWNER);
    runtime
        .block_on(owner.submit_manual(OWNER, TOKEN, false))
        .unwrap();
    assert!(owner.with_token(OTHER, |_| ()).is_err());
    owner.shutdown();
    assert!(owner.with_token(OWNER, |_| ()).is_err());
    assert_eq!(owner.status().state, DiscordSessionState::Disconnected);
}

#[test]
fn shutdown_during_reserved_browser_capture_rejects_the_late_token() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (owner, store) = owner(OWNER);
    let attempt = owner.begin_install(OWNER).unwrap();
    assert_eq!(owner.status().state, DiscordSessionState::Verifying);

    owner.shutdown();
    let result =
        runtime.block_on(owner.install_captured(attempt, Zeroizing::new(TOKEN.into()), true));

    assert!(result.is_err());
    assert_eq!(owner.status().state, DiscordSessionState::Disconnected);
    assert!(owner.with_token(OWNER, |_| ()).is_err());
    assert!(store.credential.lock().unwrap().is_none());
}

#[test]
fn forgetting_during_verification_cannot_restore_or_persist_stale_authority() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Arc::new(MemoryStore::default());
        let client = Arc::new(DelayedIdentityClient {
            identity: VerifiedDiscordIdentity {
                account_id: OWNER.into(),
                username: "synthetic_user".into(),
                display_name: "Synthetic User".into(),
            },
            started: Notify::new(),
            release: Notify::new(),
        });
        let session = Arc::new(DiscordSessionOwner::with_dependencies(
            client.clone(),
            store.clone(),
        ));
        let pending = {
            let session = session.clone();
            tokio::spawn(async move { session.submit_manual(OWNER, TOKEN, true).await })
        };

        client.started.notified().await;
        session.forget().unwrap();
        client.release.notify_one();

        assert!(pending.await.unwrap().is_err());
        assert_eq!(session.status().state, DiscordSessionState::Disconnected);
        assert!(session.with_token(OWNER, |_| ()).is_err());
        assert!(store.credential.lock().unwrap().is_none());
    });
}

#[test]
fn forgetting_during_keychain_load_invalidates_the_reserved_restore_attempt() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Arc::new(BlockingLoadStore::new());
        let client = Arc::new(IdentityClient {
            identity: VerifiedDiscordIdentity {
                account_id: OWNER.into(),
                username: "synthetic_user".into(),
                display_name: "Synthetic User".into(),
            },
        });
        let session = Arc::new(DiscordSessionOwner::with_dependencies(
            client,
            store.clone(),
        ));
        let pending = {
            let session = session.clone();
            tokio::spawn(async move { session.load_remembered(OWNER).await })
        };

        store.started.notified().await;
        let state_during_load = session.status().state;
        session.forget().unwrap();
        store.release_load();

        assert_eq!(state_during_load, DiscordSessionState::Verifying);
        assert!(pending.await.unwrap().is_err());
        assert_eq!(session.status().state, DiscordSessionState::Disconnected);
        assert!(session.with_token(OWNER, |_| ()).is_err());
        assert!(store.credential.lock().unwrap().is_none());
        assert_eq!(store.loads.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn forget_return_cannot_be_followed_by_a_restore_registered_before_forget() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Arc::new(ForgetBarrierStore::new());
        let registration = Arc::new(RegistrationBarrier::new());
        let client = Arc::new(IdentityClient {
            identity: VerifiedDiscordIdentity {
                account_id: OWNER.into(),
                username: "synthetic_user".into(),
                display_name: "Synthetic User".into(),
            },
        });
        let session = Arc::new(DiscordSessionOwner::with_dependencies(
            client,
            store.clone(),
        ));
        let restore = {
            let session = session.clone();
            let registration = registration.clone();
            tokio::spawn(async move {
                session
                    .load_remembered_with_registration_hook(OWNER, move || registration.wait())
                    .await
            })
        };
        registration.registered.notified().await;

        let forget = {
            let session = session.clone();
            tokio::task::spawn_blocking(move || session.forget())
        };
        store.forget_started.notified().await;
        registration.release();
        let restore_result = tokio::time::timeout(std::time::Duration::from_secs(2), restore)
            .await
            .unwrap()
            .unwrap();
        store.release_forget();
        forget.await.unwrap().unwrap();

        assert!(restore_result.is_err());
        assert_eq!(session.status().state, DiscordSessionState::Disconnected);
        assert!(session.with_token(OWNER, |_| ()).is_err());
        assert!(store.credential.lock().unwrap().is_none());
    });
}
