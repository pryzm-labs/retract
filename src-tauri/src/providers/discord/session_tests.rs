use super::session::{
    DiscordCredentialStore, DiscordIdentityClient, DiscordSessionOwner, DiscordSessionState,
    StoredDiscordCredential, VerifiedDiscordIdentity,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

const OWNER: &str = "9007199254741001";
const OTHER: &str = "9007199254741002";
const TOKEN: &str = "synthetic.discord.token-value_123456789";

#[derive(Default)]
struct MemoryStore(Mutex<Option<(String, String)>>);

impl DiscordCredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<StoredDiscordCredential>, crate::error::AppError> {
        Ok(self.0.lock().unwrap().as_ref().map(|(account_id, token)| {
            StoredDiscordCredential::new(account_id.clone(), Zeroizing::new(token.clone())).unwrap()
        }))
    }

    fn save(&self, credential: &StoredDiscordCredential) -> Result<(), crate::error::AppError> {
        self.0.lock().unwrap().replace((
            credential.account_id().to_owned(),
            credential.expose_token_for_request(|value| value.to_owned()),
        ));
        Ok(())
    }

    fn forget(&self) -> Result<(), crate::error::AppError> {
        self.0.lock().unwrap().take();
        Ok(())
    }
}

struct IdentityClient {
    identity: VerifiedDiscordIdentity,
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
    assert!(store.0.lock().unwrap().is_none());
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
    assert!(mismatch_store.0.lock().unwrap().is_none());

    let (matching, matching_store) = owner(OWNER);
    let status = runtime
        .block_on(matching.submit_manual(OWNER, TOKEN, true))
        .unwrap();
    assert!(status.remembered);
    assert_eq!(matching_store.0.lock().unwrap().as_ref().unwrap().0, OWNER);
    matching.forget().unwrap();
    assert_eq!(matching.status().state, DiscordSessionState::Disconnected);
    assert!(matching_store.0.lock().unwrap().is_none());
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
