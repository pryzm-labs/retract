use super::{
    ArchiveError, ArchiveOwner, ArchiveQuerySource, ArchiveService, ArchiveStore, ImportPhase,
    lifecycle::prepare_archive_directory,
    test_support::{Fixture, account, batch, key, source, validators},
    worker_tests::{Gate, Release, runtime},
};
use crate::{
    error::AppError,
    providers::ports::{ContentQuery, ConversationQuery, QuerySource, ResolveRequest},
    secure_store::vault_lock_tests::InjectedApplicationVault,
};
use std::{
    fs,
    sync::{Arc, atomic::Ordering},
};

#[test]
fn lifecycle_is_lazy_reuses_settings_lease_and_drains_before_terminal_clear() {
    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.directory.path()).unwrap();
    let vault = InjectedApplicationVault::new(root.clone());
    let vault_loader = vault.clone();
    let app_root = root.clone();
    let gate = Arc::new(Gate::default());
    let blocking = gate.clone();
    let owner = ArchiveOwner::with_opener(move || {
        let path = prepare_archive_directory(&app_root)?.join("content.db");
        let mut store = ArchiveStore::open_with_key_loader(path, validators(), || {
            vault_loader.key().map_err(|_| ArchiveError::UnavailableKey)
        })?;
        let hook = blocking.clone();
        store.before_commit = Some(Box::new(move || hook.wait()));
        Ok(store)
    });
    assert_eq!(vault.calls(), 0);
    assert!(!root.join("archives").exists());
    // Existing Telegram setup/bootstrap performs no archive operation.
    drop(crate::provider_service::ProviderService::setup());
    assert_eq!(vault.calls(), 0);
    assert!(!root.join("archives").exists());
    vault.settings().unwrap();
    let competitor = InjectedApplicationVault::new(root.clone());
    runtime().block_on(async move {
        let _release = Release(gate.clone());
        let service = owner.open().await.unwrap();
        assert!(root.join("archives/content.db").is_file());
        assert!(Arc::ptr_eq(&service, &owner.open().await.unwrap()));
        assert!(matches!(competitor.settings(), Err(AppError::ProfileInUse)));
        assert_eq!(competitor.calls(), 0);
        vault.settings().unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let session = service.begin_import(&source().scope()).await.unwrap();
        gate.armed.store(true, Ordering::Release);
        let active = service
            .append_batch(&session, 0, &batch("1", "synthetic"))
            .unwrap();
        gate.entered().await;
        let mut shutdown = Box::pin(owner.shutdown());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        drop(shutdown);
        assert!(matches!(competitor.settings(), Err(AppError::ProfileInUse)));
        let mut resumed = Box::pin(owner.shutdown());
        assert!(futures_util::poll!(resumed.as_mut()).is_pending());
        gate.release();
        resumed.await;
        assert_eq!(active.await.unwrap().unwrap().committed_items, 1);
        assert_eq!(
            owner.open().await.err().map(|error| error.code()),
            Some("cancelled")
        );
        assert_eq!(
            service.source(&source().scope()).await.err(),
            Some(ArchiveError::Cancelled)
        );
        // An external Arc no longer owns a running repository at this point.
        let path = root.join("archives/content.db");
        drop(
            ArchiveStore::open_with_key_loader(path, validators(), || {
                vault.key().map_err(|_| ArchiveError::UnavailableKey)
            })
            .unwrap(),
        );
        vault.clear();
        let calls = vault.calls();
        assert!(vault.settings().is_err());
        assert_eq!(vault.calls(), calls);
        competitor.settings().unwrap();
    });
}

#[test]
fn lifecycle_cancelled_open_and_shutdown_waiters_retain_tracked_work() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    let gate = Arc::new(Gate::default());
    gate.armed.store(true, Ordering::Release);
    let blocking = gate.clone();
    let owner = ArchiveOwner::with_opener(move || {
        blocking.wait();
        ArchiveStore::open(path.clone(), key(), validators())
    });
    runtime().block_on(async move {
        let _release = Release(gate.clone());
        let mut open = Box::pin(owner.open());
        assert!(futures_util::poll!(open.as_mut()).is_pending());
        gate.entered().await;
        drop(open);
        let mut shutdown = Box::pin(owner.shutdown());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        drop(shutdown);
        assert_eq!(owner.open().await.err(), Some(ArchiveError::Cancelled));
        let mut resumed = Box::pin(owner.shutdown());
        assert!(futures_util::poll!(resumed.as_mut()).is_pending());
        gate.release();
        resumed.await;
        drop(fixture.open());
    });
}

#[test]
fn lifecycle_first_open_failure_is_sticky_and_shutdown_never_reopens() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts = calls.clone();
    let owner = ArchiveOwner::with_opener(move || {
        attempts.fetch_add(1, Ordering::AcqRel);
        Err(ArchiveError::StoreInUse)
    });
    runtime().block_on(async {
        assert_eq!(owner.open().await.err(), Some(ArchiveError::StoreInUse));
        assert_eq!(owner.open().await.err(), Some(ArchiveError::StoreInUse));
        assert_eq!(calls.load(Ordering::Acquire), 1);
        owner.shutdown().await;
        assert_eq!(owner.open().await.err(), Some(ArchiveError::Cancelled));
        assert_eq!(calls.load(Ordering::Acquire), 1);
    });
}

#[test]
fn lifecycle_directory_preflight_rejects_alias_and_writable_parent() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let root = fs::canonicalize(fixture.directory.path()).unwrap();
    let alias = root.join("alias");
    symlink(&root, &alias).unwrap();
    assert_eq!(
        prepare_archive_directory(&alias),
        Err(ArchiveError::InvalidStore)
    );
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        prepare_archive_directory(&root),
        Err(ArchiveError::InvalidStore)
    );
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let fresh = root.join("new-app");
    let archive = prepare_archive_directory(&fresh).unwrap();
    assert_eq!(
        fs::metadata(archive).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn lifecycle_non_macos_production_open_never_creates_files() {
    let fixture = Fixture::new();
    let root = fixture.directory.path().join("absent");
    let owner = ArchiveOwner::application(root.clone());
    runtime().block_on(async {
        assert_eq!(owner.open().await.err(), Some(ArchiveError::UnavailableKey));
        owner.shutdown().await;
    });
    assert!(!root.exists());
}

fn other_source() -> retract_domain::SourceRecord {
    let mut other = source();
    other.id = uuid::Uuid::parse_str("33333333-3333-4333-8333-333333333333")
        .unwrap()
        .try_into()
        .unwrap();
    other.archive_fingerprint = Some("synthetic-export-02".into());
    other
}
fn rescope(mut input: super::ImportBatch, scope: &retract_domain::Scope) -> super::ImportBatch {
    for actor in &mut input.actors {
        actor.scope = scope.clone();
    }
    for conversation in &mut input.conversations {
        conversation.scope = scope.clone();
        for actor in &mut conversation.participants {
            actor.scope = scope.clone();
        }
    }
    for content in &mut input.contents {
        content.scope = scope.clone();
    }
    input
}

#[test]
fn lifecycle_real_query_ports_survive_restart_retry_and_source_removal() {
    let fixture = Fixture::new();
    runtime().block_on(async {
        let path = fixture.path.clone();
        let service = ArchiveService::open(move || ArchiveStore::open(path, key(), validators()))
            .await
            .unwrap();
        let other = other_source();
        for source in [source(), other.clone()] {
            service.register_source(&account(), &source).await.unwrap();
            let session = service.begin_import(&source.scope()).await.unwrap();
            let input = rescope(batch("1", "archived unique phrase"), &source.scope());
            service
                .append_batch(&session, 0, &input)
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            if source.id == other.id {
                service.finish_import(&session).await.unwrap();
            }
        }
        service.shutdown().await;
        let path = fixture.path.clone();
        let service = ArchiveService::open(move || ArchiveStore::open(path, key(), validators()))
            .await
            .unwrap();
        let checkpoint = service
            .import_status(
                &source().scope(),
                "synthetic-export-01",
                &source().schema_profile,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.progress.phase, ImportPhase::Interrupted);
        let retry = service.retry_import(&checkpoint).await.unwrap();
        assert_eq!(
            service
                .append_batch(&retry, 0, &batch("1", "archived unique phrase"))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .committed_items,
            1
        );
        service.finish_import(&retry).await.unwrap();
        let query = ArchiveQuerySource(service.clone());
        let request = ContentQuery {
            scope: source().scope(),
            query: "unique phrase".into(),
            cursor: None,
            limit: 200,
        };
        let found = query.search(request.clone()).await.unwrap();
        assert_eq!(found.items.len(), 1);
        assert_eq!(
            query
                .list_conversations(ConversationQuery {
                    scope: source().scope(),
                    cursor: None,
                    limit: 200
                })
                .await
                .unwrap()
                .items
                .len(),
            1
        );
        let record = &found.items[0];
        let target = retract_domain::ScopedResourceRef {
            id: *record.id.as_uuid(),
            scope: record.scope.clone(),
            resource: record.resource.clone(),
        };
        assert_eq!(
            query
                .resolve(ResolveRequest {
                    scope: source().scope(),
                    refs: vec![target.clone()]
                })
                .await
                .unwrap(),
            found.items
        );
        assert!(
            query
                .resolve(ResolveRequest {
                    scope: other.scope(),
                    refs: vec![target]
                })
                .await
                .is_err()
        );
        assert_eq!(
            service
                .remove_source(&source().scope())
                .await
                .unwrap()
                .removed_items,
            1
        );
        assert!(query.search(request).await.is_err());
        let surviving = query
            .search(ContentQuery {
                scope: other.scope(),
                query: "unique phrase".into(),
                cursor: None,
                limit: 200,
            })
            .await
            .unwrap();
        assert_eq!(surviving.items.len(), 1);
        assert_eq!(surviving.items[0].id, record.id);
        assert!(
            service
                .register_source(&account(), &source())
                .await
                .is_err()
        );
        assert!(service.finish_import(&retry).await.is_err());
        assert!(
            !service
                .retry_cleanup(&source().scope())
                .await
                .unwrap()
                .maintenance_pending
        );
        service.shutdown().await;
    });
}
