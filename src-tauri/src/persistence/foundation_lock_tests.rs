use std::sync::Arc;

use serde_json::json;

use super::*;

fn binding() -> StoreBinding {
    StoreBinding {
        provider: serde_json::from_value(json!("telegram")).unwrap(),
        profile: "telegram-compatibility".into(),
    }
}

#[test]
fn final_store_owner_explicitly_releases_lock_with_retained_description() {
    let directory = tempfile::tempdir().unwrap();
    let profile = directory.path().to_path_buf();
    let binding = binding();
    let store =
        FoundationStore::open_with_test_key(profile.clone(), binding.clone(), [7; KEY_LENGTH])
            .unwrap();
    let other_owner = Arc::clone(&store);
    let retained = store._profile_lock.0.try_clone().unwrap();

    drop(store);
    assert!(matches!(
        FoundationStore::open_independent_with_test_key_loader(
            profile.clone(),
            binding.clone(),
            |_| panic!("lock must precede key"),
        ),
        Err(AppError::ProfileInUse)
    ));

    drop(other_owner);
    let reopened = FoundationStore::open_with_test_key(profile, binding, [7; KEY_LENGTH]);
    assert!(
        reopened.is_ok(),
        "final store owner must release its lock: {reopened:?}"
    );
    drop(retained);
}

#[test]
fn failed_initialization_releases_lock_without_changing_active_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let profile = prepare_profile(directory.path()).unwrap();
    let active_path = profile.join(ACTIVE_FILE);
    let active_bytes = b"invalid active store".to_vec();
    fs::write(&active_path, &active_bytes).unwrap();
    let profile_lock = acquire_profile_lock(&profile).unwrap();
    let retained = profile_lock.0.try_clone().unwrap();

    let failed = FoundationStore::build(
        profile.clone(),
        binding(),
        [7; KEY_LENGTH],
        Arc::new(RealStoreIo),
        ValidationPolicy::Rejecting,
        Arc::new(RejectProviderPayloads),
        profile_lock,
    );

    assert!(failed.is_err());
    assert_eq!(fs::read(&active_path).unwrap(), active_bytes);
    let reacquired = acquire_profile_lock(&profile);
    assert!(
        reacquired.is_ok(),
        "failed initialization must release its acquired profile lock"
    );
    drop(reacquired);
    drop(retained);
}
