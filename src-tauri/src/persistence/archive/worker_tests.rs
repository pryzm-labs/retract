use super::{
    ArchiveError, ArchiveService, ArchiveStore,
    test_support::{Fixture, key, validators},
};
use super::{
    ArchiveSearch, ImportBatch, ImportPhase,
    test_support::{account, attachment, batch, envelope, source},
};
use std::sync::{Condvar, Mutex};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
pub(super) struct Gate {
    pub armed: AtomicBool,
    pub entered: AtomicBool,
    released: Mutex<bool>,
    condition: Condvar,
}
impl Gate {
    pub fn wait(&self) {
        if self.armed.swap(false, Ordering::AcqRel) {
            self.entered.store(true, Ordering::Release);
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.condition.wait(released).unwrap();
            }
        }
    }
    pub fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.condition.notify_all();
    }
    pub async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.entered.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
}
pub(super) struct Release(pub Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

pub(super) fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

fn oversized_envelope() -> retract_domain::VersionedPayload {
    let overhead = serde_json::to_vec(&envelope(String::new())).unwrap().len();
    let value = envelope("x".repeat(super::model::ENVELOPE_BYTES + 1 - overhead));
    assert_eq!(
        serde_json::to_vec(&value).unwrap().len(),
        super::model::ENVELOPE_BYTES + 1
    );
    value
}

fn make_resource_oversized(resource: &mut retract_domain::ProviderResourceRef) {
    resource.locator_payload = serde_json::json!({"padding": ""});
    let overhead = serde_json::to_vec(resource).unwrap().len();
    resource.locator_payload = serde_json::json!({
        "padding": "x".repeat(super::model::ENVELOPE_BYTES + 1 - overhead)
    });
    assert_eq!(
        serde_json::to_vec(resource).unwrap().len(),
        super::model::ENVELOPE_BYTES + 1
    );
}

fn oversized_item_batches() -> Vec<(&'static str, ImportBatch)> {
    let mut text_and_names = batch("oversized-text-and-names", "");
    text_and_names.contents[0].searchable_text = "x".repeat(super::model::MAX_SEARCHABLE_BYTES);
    text_and_names.contents[0].attachments.push(attachment("x"));

    let mut attachment_count = batch("oversized-attachment-count", "body");
    attachment_count.contents[0].attachments =
        vec![attachment("name"); super::model::MAX_ATTACHMENTS + 1];

    let mut content_resource = batch("oversized-content-resource", "body");
    make_resource_oversized(&mut content_resource.contents[0].resource);
    let mut content_metadata = batch("oversized-content-metadata", "body");
    content_metadata.contents[0].provider_metadata = Some(oversized_envelope());
    let mut attachment_locator = batch("oversized-attachment-locator", "body");
    let mut oversized_attachment = attachment("name");
    oversized_attachment.locator = oversized_envelope();
    attachment_locator.contents[0]
        .attachments
        .push(oversized_attachment);

    let mut conversation_resource = batch("oversized-conversation-resource", "body");
    make_resource_oversized(&mut conversation_resource.conversations[0].resource);
    let mut conversation_metadata = batch("oversized-conversation-metadata", "body");
    conversation_metadata.conversations[0].provider_metadata = Some(oversized_envelope());

    let mut actor_resource = batch("oversized-actor-resource", "body");
    make_resource_oversized(&mut actor_resource.actors[0].resource);
    let mut actor_avatar = batch("oversized-actor-avatar", "body");
    actor_avatar.actors[0].avatar = Some(oversized_envelope());

    let mut nested_resource = batch("oversized-nested-resource", "body");
    let mut nested = nested_resource.actors[0].clone();
    make_resource_oversized(&mut nested.resource);
    nested_resource.conversations[0].participants.push(nested);
    let mut nested_avatar = batch("oversized-nested-avatar", "body");
    let mut nested = nested_avatar.actors[0].clone();
    nested.avatar = Some(oversized_envelope());
    nested_avatar.conversations[0].participants.push(nested);

    vec![
        ("searchable text plus attachment names", text_and_names),
        ("attachment count", attachment_count),
        ("content resource locator", content_resource),
        ("content provider metadata", content_metadata),
        ("attachment locator", attachment_locator),
        ("conversation resource locator", conversation_resource),
        ("conversation provider metadata", conversation_metadata),
        ("actor resource locator", actor_resource),
        ("actor avatar", actor_avatar),
        ("nested actor resource locator", nested_resource),
        ("nested actor avatar", nested_avatar),
    ]
}

#[test]
fn worker_open_runs_off_runtime_and_retains_repository_lock() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let timer = Arc::new(AtomicBool::new(false));
        let seen = timer.clone();
        let tick = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            seen.store(true, Ordering::Release);
        });
        let opening = ArchiveService::open(move || {
            std::thread::sleep(Duration::from_millis(150));
            assert!(
                timer.load(Ordering::Acquire),
                "opening store blocked the Tokio timer"
            );
            ArchiveStore::open(path, key(), validators())
        });
        let service = opening.await.unwrap();
        tick.await.unwrap();
        assert_eq!(
            ArchiveStore::open(fixture.path.clone(), key(), validators()).err(),
            Some(ArchiveError::StoreInUse)
        );
        service.shutdown().await;
        drop(fixture.open());
    });
}

#[test]
fn worker_shutdown_wakes_with_reserved_unsent_slots_after_waiter_cancellation() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        let _release = Release(gate.clone());
        let blocking = gate.clone();
        let service = ArchiveService::open(move || {
            let mut store = ArchiveStore::open(path, key(), validators())?;
            store.before_commit = Some(Box::new(move || blocking.wait()));
            Ok(store)
        })
        .await
        .unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let session = service.begin_import(&source().scope()).await.unwrap();
        gate.armed.store(true, Ordering::Release);
        let active = service
            .append_batch(&session, 0, &batch("1", "in flight"))
            .unwrap();
        gate.entered().await;
        let first = super::worker::reserve_command_slot(&service);
        let second = super::worker::reserve_command_slot(&service);
        let mut shutdown = Box::pin(service.shutdown());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        drop(shutdown);
        drop(first);
        drop(second);
        gate.release();
        let drained = tokio::time::timeout(Duration::from_millis(500), service.shutdown()).await;
        // Drop can wake the original buggy worker after permits are free; do
        // this before asserting so the RED regression cannot hang teardown.
        if drained.is_err() {
            drop(service);
            panic!("shutdown lost its wakeup while both empty queue slots were reserved");
        }
        active.await.unwrap().unwrap();
        drop(fixture.open());
        assert_eq!(
            service.source(&source().scope()).await.err(),
            Some(ArchiveError::Cancelled)
        );
    });
}

#[test]
fn worker_full_queue_backpressures_and_cancellation_preempts_queued_mutations() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        let _release = Release(gate.clone());
        let blocking = gate.clone();
        let service = ArchiveService::open(move || {
            let mut store = ArchiveStore::open(path, key(), validators())?;
            store.before_commit = Some(Box::new(move || blocking.wait()));
            Ok(store)
        })
        .await
        .unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let session = service.begin_import(&source().scope()).await.unwrap();
        let first = batch("1", "committed");
        assert_eq!(
            service
                .append_batch(&session, 0, &first)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .committed_items,
            1
        );
        gate.armed.store(true, Ordering::Release);
        let mut active = service
            .append_batch(&session, 1, &batch("2", "rolled back"))
            .unwrap();
        gate.entered().await;
        let second = service
            .append_batch(&session, 2, &batch("3", "never started"))
            .unwrap();
        let third = service
            .append_batch(&session, 3, &batch("4", "never started"))
            .unwrap();
        let rejected = batch("5", "caller retains this batch");
        assert_eq!(
            service.append_batch(&session, 4, &rejected).err(),
            Some(ArchiveError::Busy)
        );
        assert_eq!(
            rejected.contents[0].searchable_text,
            "caller retains this batch"
        );
        let oversized = ImportBatch {
            contents: vec![first.contents[0].clone(); 501],
            ..Default::default()
        };
        assert_eq!(
            service.append_batch(&session, 4, &oversized).err(),
            Some(ArchiveError::LimitExceeded)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(matches!(
            active.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        let mut cancel = Box::pin(service.cancel_import(&session));
        assert!(futures_util::poll!(cancel.as_mut()).is_pending());
        assert_eq!(
            session.cancellation_signal().check(),
            Err(ArchiveError::Cancelled)
        );
        gate.release();
        assert_eq!(active.await.unwrap(), Err(ArchiveError::Cancelled));
        assert_eq!(second.await.unwrap(), Err(ArchiveError::Cancelled));
        assert_eq!(third.await.unwrap(), Err(ArchiveError::Cancelled));
        let progress = cancel.await.unwrap();
        assert_eq!(progress.phase, ImportPhase::Cancelled);
        assert_eq!(progress.committed_items, 1);
        assert_eq!(progress.next_batch, 1);
        service.shutdown().await;
        let mut reopened = fixture.open();
        let checkpoint = reopened
            .import_status(
                &source().scope(),
                "synthetic-export-01",
                &source().schema_profile,
            )
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.progress, progress);
        let retry = reopened.retry_import(&checkpoint).unwrap();
        assert_eq!(reopened.finish_import(&retry).unwrap().committed_items, 1);
    });
}

#[test]
fn worker_rejects_every_per_item_limit_synchronously_with_free_capacity() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let service = ArchiveService::open(move || ArchiveStore::open(path, key(), validators()))
            .await
            .unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let session = service.begin_import(&source().scope()).await.unwrap();
        for (name, input) in oversized_item_batches() {
            assert!(
                serde_json::to_vec(&input).unwrap().len() < super::model::MAX_BATCH_BYTES,
                "{name} must stay below the total batch cap"
            );
            match service.append_batch(&session, 0, &input) {
                Err(error) => assert_eq!(error, ArchiveError::LimitExceeded, "{name}"),
                Ok(result) => {
                    assert_eq!(result.await.unwrap(), Err(ArchiveError::LimitExceeded));
                    panic!("{name} was accepted into the worker queue");
                }
            }
        }
        service.shutdown().await;
    });
}

#[test]
fn worker_rejects_every_per_item_limit_before_a_deterministically_full_queue() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        let _release = Release(gate.clone());
        let blocking = gate.clone();
        let service = ArchiveService::open(move || {
            let mut store = ArchiveStore::open(path, key(), validators())?;
            store.before_commit = Some(Box::new(move || blocking.wait()));
            Ok(store)
        })
        .await
        .unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let session = service.begin_import(&source().scope()).await.unwrap();
        gate.armed.store(true, Ordering::Release);
        let active = service
            .append_batch(&session, 0, &batch("full-0", "active"))
            .unwrap();
        gate.entered().await;
        let queued_one = service
            .append_batch(&session, 1, &batch("full-1", "queued"))
            .unwrap();
        let queued_two = service
            .append_batch(&session, 2, &batch("full-2", "queued"))
            .unwrap();

        for (name, input) in oversized_item_batches() {
            assert!(
                serde_json::to_vec(&input).unwrap().len() < super::model::MAX_BATCH_BYTES,
                "{name} must stay below the total batch cap"
            );
            assert_eq!(
                service.append_batch(&session, 3, &input).err(),
                Some(ArchiveError::LimitExceeded),
                "{name}"
            );
        }
        gate.release();
        active.await.unwrap().unwrap();
        queued_one.await.unwrap().unwrap();
        queued_two.await.unwrap().unwrap();
        service.shutdown().await;
    });
}

#[test]
fn worker_shutdown_cancels_unstarted_commands_and_releases_lock() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        let _release = Release(gate.clone());
        let blocking = gate.clone();
        let service = ArchiveService::open(move || {
            let mut store = ArchiveStore::open(path, key(), validators())?;
            store.before_commit = Some(Box::new(move || blocking.wait()));
            Ok(store)
        })
        .await
        .unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let session = service.begin_import(&source().scope()).await.unwrap();
        gate.armed.store(true, Ordering::Release);
        let active = service
            .append_batch(&session, 0, &batch("1", "in flight"))
            .unwrap();
        gate.entered().await;
        let queued = service
            .append_batch(&session, 1, &batch("2", "unstarted"))
            .unwrap();
        let mut shutdown = Box::pin(service.shutdown());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        assert_eq!(
            service.begin_import(&source().scope()).await.err(),
            Some(ArchiveError::Cancelled)
        );
        gate.release();
        shutdown.await;
        assert_eq!(active.await.unwrap().unwrap().committed_items, 1);
        assert!(queued.await.is_err());
        let store = fixture.open();
        assert_eq!(
            store
                .import_status(
                    &source().scope(),
                    "synthetic-export-01",
                    &source().schema_profile
                )
                .unwrap()
                .unwrap()
                .progress
                .committed_items,
            1
        );
    });
}

pub(super) fn search(scope: retract_domain::Scope, text: &str) -> ArchiveSearch {
    ArchiveSearch {
        scope,
        text: text.into(),
        kinds: vec![],
        author: None,
        before: None,
        after: None,
        cursor: None,
        limit: 200,
    }
}

#[test]
fn worker_rejects_oversized_requests_before_a_full_queue() {
    use crate::providers::ports::{ConversationQuery, ResolveRequest};
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        gate.armed.store(true, Ordering::Release);
        let _release = Release(gate.clone());
        let blocking = gate.clone();
        let service = ArchiveService::start(move || {
            blocking.wait();
            ArchiveStore::open(path, key(), validators())
        });
        gate.entered().await;
        let first_scope = source().scope();
        let mut first = Box::pin(service.source(&first_scope));
        // Keep concrete scopes alive while requests borrow them.
        assert!(futures_util::poll!(first.as_mut()).is_pending());
        let scope = source().scope();
        let mut second = Box::pin(service.source(&scope));
        assert!(futures_util::poll!(second.as_mut()).is_pending());
        let bounded = async {
            assert_eq!(
                service
                    .search(&search(scope.clone(), &"x".repeat(4097)))
                    .await
                    .err(),
                Some(ArchiveError::LimitExceeded)
            );
            assert_eq!(
                service
                    .list_conversations(&ConversationQuery {
                        scope: scope.clone(),
                        cursor: Some("x".repeat(4097)),
                        limit: 1
                    })
                    .await
                    .err(),
                Some(ArchiveError::StaleCursor)
            );
            let mut huge = account();
            huge.display_name = "x".repeat(4 * 1024 * 1024);
            assert_eq!(
                service.register_source(&huge, &source()).await.err(),
                Some(ArchiveError::LimitExceeded)
            );
            let mut schema = source().schema_profile;
            schema.payload = serde_json::json!({"huge": "x".repeat(65537)});
            assert_eq!(
                service
                    .import_status(&scope, "synthetic-export-01", &schema)
                    .await
                    .err(),
                Some(ArchiveError::LimitExceeded)
            );
            let checkpoint = super::ImportCheckpoint {
                scope: scope.clone(),
                fingerprint: "synthetic-export-01".into(),
                schema_profile: schema,
                run_id: uuid::Uuid::new_v4(),
                revision: 1,
                progress: super::ImportProgress {
                    phase: ImportPhase::Interrupted,
                    committed_items: 0,
                    committed_bytes: 0,
                    next_batch: 0,
                },
                warnings: vec![],
            };
            assert_eq!(
                service.retry_import(&checkpoint).await.err(),
                Some(ArchiveError::LimitExceeded)
            );
            let mut target = batch("1", "data").contents.remove(0);
            target.resource.locator_payload = serde_json::json!({"huge": "x".repeat(65537)});
            let refs = vec![retract_domain::ScopedResourceRef {
                id: *target.id.as_uuid(),
                scope: scope.clone(),
                resource: target.resource,
            }];
            assert_eq!(
                service
                    .resolve(&ResolveRequest {
                        scope: scope.clone(),
                        refs
                    })
                    .await
                    .err(),
                Some(ArchiveError::LimitExceeded)
            );
        };
        tokio::time::timeout(Duration::from_secs(1), bounded)
            .await
            .unwrap();
        gate.release();
        service.shutdown().await;
    });
}

#[test]
fn worker_removal_invalidates_previously_queued_session_authority() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        let _release = Release(gate.clone());
        let blocking = gate.clone();
        let service = ArchiveService::open(move || {
            let mut store = ArchiveStore::open(path, key(), validators())?;
            store.before_maintenance = Some(Box::new(move |_| {
                blocking.wait();
                Ok(())
            }));
            Ok(store)
        })
        .await
        .unwrap();
        service
            .register_source(&account(), &source())
            .await
            .unwrap();
        let scope = source().scope();
        let session = service.begin_import(&scope).await.unwrap();
        service
            .append_batch(&session, 0, &batch("1", "remove"))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        gate.armed.store(true, Ordering::Release);
        let mut removal = Box::pin(service.remove_source(&scope));
        assert!(futures_util::poll!(removal.as_mut()).is_pending());
        gate.entered().await;
        let stale = service
            .append_batch(&session, 1, &batch("2", "must not return"))
            .unwrap();
        gate.release();
        assert_eq!(removal.await.unwrap().removed_items, 1);
        assert!(stale.await.unwrap().is_err());
        assert_eq!(
            service.source(&scope).await.err(),
            Some(ArchiveError::ScopeMismatch)
        );
        service.shutdown().await;
    });
}
