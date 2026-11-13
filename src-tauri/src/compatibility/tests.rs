//! Execute the production registration through Tauri's synthetic runtime.
use serde_json::{Value, json};
use std::sync::Arc;
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};

use crate::{compatibility::commands_v2, provider_service::ProviderService};

#[test]
fn compatibility_v2_content_supplies_the_exact_scoped_actor_for_sender_review() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../src/test/fixtures/telegram-ipc-contract.json"
    ))
    .unwrap();
    let message: cleaner_domain::MessageSnapshot =
        serde_json::from_value(fixture["searchResponse"]["messages"][0].clone()).unwrap();
    let scope = super::fixtures::context("context").scope;
    let record =
        crate::providers::telegram::normalize::normalize_content(&scope, &message).unwrap();
    let metadata = record.provider_metadata.unwrap().payload;
    let actor = &metadata["actor"];
    assert_eq!(actor["scope"], serde_json::to_value(&scope).unwrap());
    assert_eq!(actor["id"], serde_json::to_value(record.author_id).unwrap());
    assert_eq!(actor["resource"]["resourceKind"], "actor");
    assert_eq!(
        actor["resource"]["locatorPayload"],
        json!({"kind":"user","nativeId":"42"})
    );
    let reference: retract_domain::ScopedResourceRef =
        serde_json::from_value(actor.clone()).unwrap();
    reference.validate(&scope).unwrap();
}

fn setup() -> tauri::WebviewWindow<MockRuntime> {
    with_service(ProviderService::setup())
}

#[test]
fn focused_cancelled_search_retains_retiring_worker_until_replacement_can_recover() {
    use super::fixtures::*;
    use futures_util::FutureExt;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let connection = RolloverConnection::new(directory.path(), io.clone());
        let harness = Harness::new(connection.clone());
        let barrier = Arc::new(PreflightBarrier::default());
        *io.mutation_barrier.lock().unwrap() = Some(barrier.clone());
        let plan = harness.prepare(&active, vec![fixture()["messages"][2]["ref"].clone()]);
        harness.authorize(&active, &plan);
        harness.start(&active, &plan).unwrap();
        tokio::time::timeout(Duration::from_secs(2), barrier.entered.notified())
            .await
            .unwrap();
        assert!(harness.service.has_workers().await);

        let mut next = active.clone();
        next.session_generation = uuid::Uuid::new_v4();
        *io.active.write().unwrap() = Some(next.clone());
        let token = json!({"clientId":uuid::Uuid::new_v4(),"sequence":1});
        let flight = harness.service.prepare_search(json!({"contractVersion":2,"context":next,"payload":{"query":"","conversations":[],"filters":null,"limit":500,"searchToken":token}})).unwrap();
        let mut search = Box::pin(harness.service.run_search(flight));
        // All registration locks are free. This single poll enters the real
        // lifecycle stop, whose first suspension waits for the blocked worker.
        assert!(search.as_mut().now_or_never().is_none());
        assert_eq!(connection.constructions.load(Ordering::SeqCst), 1);
        harness
            .service
            .cancel_search(json!({"contractVersion":2,"context":next,"payload":token}))
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), search)
                .await
                .unwrap()
                .unwrap_err()
                .code,
            retract_domain::ErrorCode::SearchCancelled
        );
        let retained = harness.service.has_workers().await;
        // Release on RED too, so a failed ownership assertion leaves no worker
        // holding the fixture store after this test returns.
        if !retained {
            barrier.release.notify_one();
        }
        assert!(
            retained,
            "cancelled retirement must retain the old live worker"
        );

        let bootstrap = json!({"contractVersion":2,"context":next,"payload":{}});
        let mut replacement = Box::pin(harness.service.bootstrap(bootstrap.clone()));
        assert!(replacement.as_mut().now_or_never().is_none());
        assert_eq!(connection.constructions.load(Ordering::SeqCst), 1);
        // A second dropped stop waiter must not consume the drain condition.
        drop(replacement);
        assert!(harness.service.has_workers().await);
        let mut replacement = Box::pin(harness.service.bootstrap(bootstrap));
        assert!(replacement.as_mut().now_or_never().is_none());
        assert_eq!(connection.constructions.load(Ordering::SeqCst), 1);
        barrier.release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), replacement)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connection.constructions.load(Ordering::SeqCst), 2);
        assert!(!harness.service.has_workers().await);
        harness
            .call(
                "search_messages_v2",
                &next,
                json!({"query":"","conversations":[],"filters":null,"limit":500}),
            )
            .unwrap();
        let fresh = harness.prepare(&next, vec![fixture()["messages"][2]["ref"].clone()]);
        harness.authorize(&next, &fresh);
        let job = harness.start(&next, &fresh).unwrap();
        assert_eq!(harness.settled(&next, job.id).await.counters.deleted, 1);
        assert_eq!(io.calls.lock().unwrap().len(), 2);
        harness.service.shutdown().await;
    });
}

#[test]
fn focused_search_early_cancel_and_late_start_are_fenced() {
    use super::fixtures::*;
    let directory = tempfile::tempdir().unwrap();
    let active = context("syntheticContext");
    let harness = synthetic(directory.path(), Arc::new(SyntheticIo::new(active.clone())));
    let client = uuid::Uuid::new_v4();
    let token = |sequence| json!({"clientId":client,"sequence":sequence});
    harness.call("cancel_search_v2", &active, token(2)).unwrap();
    let request = |sequence| json!({"query":"","conversations":[],"filters":null,"limit":500,"searchToken":token(sequence)});
    for sequence in [1, 2] {
        assert_eq!(
            harness
                .call("search_messages_v2", &active, request(sequence))
                .unwrap_err()["code"],
            "search_cancelled"
        );
    }
    assert!(
        harness
            .call("search_messages_v2", &active, request(3))
            .is_ok()
    );
    assert!(harness.call("cancel_search_v2", &active, token(1)).is_ok());
    assert!(
        harness
            .call("search_messages_v2", &active, request(4))
            .is_ok()
    );
    let foreign = context("context");
    assert_eq!(
        harness
            .call("cancel_search_v2", &foreign, token(100))
            .unwrap_err()["code"],
        "scope_mismatch"
    );
    assert!(
        harness
            .call("search_messages_v2", &active, request(5))
            .is_ok()
    );
    let mut stale_context = active.clone();
    stale_context.session_generation = uuid::Uuid::new_v4();
    assert_eq!(
        harness
            .call("cancel_search_v2", &stale_context, token(100))
            .unwrap_err()["code"],
        "stale_context"
    );
    for invalid in [
        json!({"clientId":client,"sequence":0}),
        json!({"clientId":client,"sequence":9_007_199_254_740_992u64}),
        json!({"clientId":"invalid","sequence":99}),
    ] {
        assert!(harness.call("cancel_search_v2", &active, invalid).is_err());
    }
    assert!(
        harness
            .call("search_messages_v2", &active, request(6))
            .is_ok()
    );
}

#[test]
fn focused_blocked_search_releases_runtime_and_late_cancel_preserves_newer_read() {
    use super::fixtures::*;
    use tauri::Manager;
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let harness = synthetic(directory.path(), io.clone());
        let barrier = Arc::new(PreflightBarrier::default());
        *io.search_barrier.lock().unwrap() = Some(barrier.clone());
        let token = json!({"clientId":uuid::Uuid::new_v4(),"sequence":2});
        let payload =
            json!({"query":"","conversations":[],"filters":null,"limit":500,"searchToken":token});
        let webview = harness.webview.clone();
        let request = json!({"contractVersion":2,"context":active,"payload":payload});
        let search =
            tokio::task::spawn_blocking(move || invoke(&webview, "search_messages_v2", request));
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            barrier.entered.notified(),
        )
        .await
        .unwrap();
        let runtime = harness
            .webview
            .app_handle()
            .state::<Arc<crate::RuntimeState>>()
            .inner()
            .clone();
        let current =
            tokio::time::timeout(std::time::Duration::from_secs(1), runtime.service.write()).await;
        if current.is_err() {
            barrier.release.notify_one();
            search.await.unwrap().unwrap();
            panic!("search must not retain runtime read lock");
        }
        drop(current.unwrap());
        let mut old_token = token.clone();
        old_token["sequence"] = json!(1);
        harness
            .call("cancel_search_v2", &active, old_token)
            .unwrap();
        assert!(
            !search.is_finished(),
            "old cleanup must not cancel the newer request"
        );
        harness.call("cancel_search_v2", &active, token).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), search)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(result["code"], "search_cancelled");
        assert!(io.calls.lock().unwrap().is_empty());
        harness.service.check(&active).unwrap();
    });
}

#[test]
fn focused_admitted_unpolled_search_is_cancelled_and_client_state_is_bounded() {
    use super::fixtures::*;
    tauri::async_runtime::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let harness = synthetic(directory.path(), io.clone());
        let request = json!({"contractVersion":2,"context":active,"payload":{"query":"","conversations":[],"filters":null,"limit":500}});
        let flight = harness.service.prepare_search(request.clone()).unwrap();
        harness.service.cancel_searches();
        assert_eq!(
            harness.service.run_search(flight).await.unwrap_err().code,
            retract_domain::ErrorCode::SearchCancelled
        );
        harness.service.check(&active).unwrap();
        let known_client = uuid::Uuid::new_v4();
        for index in 0..1024 {
            let client = if index == 0 {
                known_client
            } else {
                uuid::Uuid::new_v4()
            };
            harness.service.cancel_search(json!({"contractVersion":2,"context":active,"payload":{"clientId":client,"sequence":1}})).unwrap();
        }
        let exhausted = harness.service.cancel_search(json!({"contractVersion":2,"context":active,"payload":{"clientId":uuid::Uuid::new_v4(),"sequence":1}})).unwrap_err();
        assert_eq!(
            exhausted.code,
            retract_domain::ErrorCode::SearchSessionLimit
        );
        let mut known = request;
        known["payload"]["searchToken"] = json!({"clientId":known_client,"sequence":2});
        let flight = harness.service.prepare_search(known).unwrap();
        assert!(harness.service.run_search(flight).await.is_ok());
        assert!(io.calls.lock().unwrap().is_empty());
    });
}

#[test]
fn final_review_bootstrap_serialization_matches_the_frontend_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../src/test/fixtures/connection-bootstrap.json"
    ))
    .unwrap();
    for (name, service) in [
        ("setup", ProviderService::setup()),
        (
            "failed",
            ProviderService::failed(crate::provider_service::safe(
                retract_domain::ErrorCode::ProfileInUse,
            )),
        ),
    ] {
        let actual = invoke(
            &with_service(service),
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":null,"payload":{}}),
        )
        .unwrap();
        assert_eq!(actual, fixture[name], "shared frontend fixture: {name}");
        assert_eq!(actual["payload"]["catalog"]["phase"], "idle", "{name}");
    }
}

fn with_service(service: Arc<ProviderService>) -> tauri::WebviewWindow<MockRuntime> {
    let app = commands_v2::register(mock_builder())
        .manage(Arc::new(crate::RuntimeState::new(service)))
        .build(mock_context(noop_assets()))
        .unwrap();
    tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap()
}

#[test]
fn compatibility_v2_failed_setup_exposes_safe_retryable_status_without_an_identity() {
    let webview = with_service(ProviderService::failed(crate::error::boundary_error(
        crate::error::AppError::Gateway("private bootstrap path /private/secret".into()),
    )));
    let bootstrap = invoke(
        &webview,
        "get_bootstrap_snapshot_v2",
        json!({"contractVersion":2,"context":null,"payload":{}}),
    )
    .unwrap();
    assert_eq!(bootstrap["context"], Value::Null);
    assert_eq!(bootstrap["payload"]["identity"]["state"], "failed");
    assert_eq!(
        bootstrap["payload"]["identity"]["diagnostic"]["code"],
        "permission_changed"
    );
    assert!(!bootstrap.to_string().contains("secret"));
    let error=invoke(&webview,"save_connection_settings_v2",json!({"contractVersion":2,"context":null,"payload":{"tdlibPath":"","apiId":0,"apiHash":"synthetic-invalid","useTestDc":false}})).unwrap_err();
    assert_eq!(error["code"], "scope_mismatch");
}

#[test]
fn final_review_failed_connection_retry_recreates_the_runtime() {
    // Empty synthetic app settings take create_service's setup branch, before
    // any vault, store or native connection. Retry must replace the failed service.
    let webview = with_service(ProviderService::failed(crate::provider_service::safe(
        retract_domain::ErrorCode::ProfileInUse,
    )));
    let recovered = invoke(
        &webview,
        "retry_identity_v2",
        json!({"contractVersion":2,"context":null,"payload":{}}),
    )
    .unwrap();
    assert_eq!(recovered["payload"]["identity"]["state"], "unavailable");
    assert_eq!(recovered["context"], Value::Null);
    assert_eq!(
        invoke(
            &webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":null,"payload":{}})
        )
        .unwrap(),
        recovered
    );
}

#[test]
fn final_review_failed_replacement_requires_discovery_before_an_explicit_corrected_save() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use tauri::Manager;
        let directory_a = tempfile::tempdir().unwrap();
        let directory_b = tempfile::tempdir().unwrap();
        let active = context("context");
        let mut next_context = active.clone();
        next_context.session_generation = uuid::Uuid::new_v4();
        let (a, gateway) = telegram(directory_a.path(), active.clone()).await;
        let (b, _) = telegram(directory_b.path(), next_context.clone()).await;
        let plan = a.prepare(&active, vec![fixture()["messages"][0]["ref"].clone()]);
        a.authorize(&active, &plan);
        let runtime = a.webview.app_handle().state::<Arc<crate::RuntimeState>>();
        let saves = std::cell::Cell::new(0);
        let request = |context: Value| json!({"contractVersion":2,"context":context,"payload":{"tdlibPath":"synthetic-library","apiId":1,"apiHash":null,"useTestDc":true}});
        let error = runtime
            .save_settings(
                request(json!(active)),
                |_| {
                    saves.set(saves.get() + 1);
                    Ok(())
                },
                || Err(crate::error::AppError::ProfileInUse),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, retract_domain::ErrorCode::ProfileInUse);
        assert!(a.service.context().is_none());
        assert!(
            a.start(&active, &plan).is_err(),
            "retired grant cannot execute"
        );
        assert!(
            runtime
                .save_settings(
                    request(json!(active)),
                    |_| panic!("stale save must not write"),
                    || panic!("stale save must not recreate")
                )
                .await
                .is_err()
        );
        let discovered = invoke(
            &a.webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":null,"payload":{}}),
        )
        .unwrap();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../src/test/fixtures/connection-bootstrap.json"
        ))
        .unwrap();
        assert_eq!(discovered, fixture["failed"]);
        let corrected = runtime
            .save_settings(
                request(discovered["context"].clone()),
                |_| {
                    saves.set(saves.get() + 1);
                    Ok(())
                },
                || Ok(b.service.clone()),
            )
            .await
            .unwrap();
        assert_eq!(corrected["context"], json!(next_context));
        assert_eq!(
            saves.get(),
            2,
            "only the two user-requested saves reach storage"
        );
        assert!(a.start(&active, &plan).is_err());
        assert!(
            a.start(&next_context, &plan).is_err(),
            "old plan is not retargeted"
        );
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn final_review_runtime_retry_recovers_after_profile_lock_release() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use crate::persistence::FoundationStore;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let owner = FoundationStore::open_with_test_key(
            directory.path().to_path_buf(),
            binding(&active),
            KEY,
        )
        .unwrap();
        let original = std::fs::read(directory.path().join("jobs.enc")).unwrap();
        let runtime = crate::RuntimeState::new(ProviderService::failed(
            crate::provider_service::safe(retract_domain::ErrorCode::ProfileInUse),
        ));
        let request = json!({"contractVersion":2,"context":null,"payload":{}});
        let create = || {
            let _store = FoundationStore::open_independent_with_test_key(
                directory.path().to_path_buf(),
                binding(&active),
                KEY,
            )?;
            Ok(ProviderService::setup())
        };
        assert_eq!(
            runtime
                .retry_identity(request.clone(), create)
                .await
                .unwrap_err()
                .code,
            retract_domain::ErrorCode::ProfileInUse
        );
        drop(owner);
        let recovered = runtime.retry_identity(request, create).await.unwrap();
        assert_eq!(recovered["payload"]["identity"]["state"], "unavailable");
        assert_eq!(
            std::fs::read(directory.path().join("jobs.enc")).unwrap(),
            original
        );
    });
}

mod discord_source_switch_tests {
    use crate::{
        compatibility::{commands_v2, tests::invoke},
        persistence::{
            FoundationStore, StoreBinding,
            archive::{ArchiveKey, ArchiveOwner, ArchiveStore},
        },
        provider_service::ProviderService,
    };
    use serde_json::json;
    use std::{fs, sync::Arc};

    struct DrainArchive<'a>(&'a tokio::runtime::Runtime, Arc<ArchiveOwner>);
    type BlockedSearch = (
        Arc<crate::compatibility::fixtures::PreflightBarrier>,
        std::sync::mpsc::Receiver<Result<serde_json::Value, serde_json::Value>>,
        std::thread::JoinHandle<()>,
    );
    impl Drop for DrainArchive<'_> {
        fn drop(&mut self) {
            self.0.block_on(self.1.shutdown());
        }
    }

    #[test]
    fn failed_discord_source_open_preserves_workspace_and_allows_retry() {
        failed_switch(false, false);
    }

    #[test]
    fn failed_discord_source_open_preserves_telegram_context_and_saved_state() {
        failed_switch(true, false);
    }

    #[test]
    fn focused_source_switch_cancels_blocked_search_and_failed_candidate_recovers() {
        failed_switch(false, true);
    }

    fn failed_switch(with_telegram: bool, with_blocked_search: bool) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let archive_path = root.join("archive.db");
        let owner = ArchiveOwner::with_opener(move || {
            ArchiveStore::open(
                archive_path.clone(),
                ArchiveKey::new([0x67; 32]),
                std::collections::BTreeMap::from([(
                    crate::providers::discord::locators::discord_provider_key(),
                    Arc::new(crate::providers::discord::DiscordPayloadValidator)
                        as Arc<dyn crate::persistence::ProviderPayloadValidator>,
                )]),
            )
        });
        let telegram_harness = if with_telegram {
            Some(
                rt.block_on(crate::compatibility::fixtures::telegram(
                    &root.join("telegram"),
                    crate::compatibility::fixtures::context("context"),
                ))
                .0,
            )
        } else {
            None
        };
        let synthetic_io = with_blocked_search.then(|| {
            Arc::new(crate::compatibility::fixtures::SyntheticIo::new(
                crate::compatibility::fixtures::context("syntheticContext"),
            ))
        });
        let synthetic_harness = synthetic_io.as_ref().map(|io| {
            crate::compatibility::fixtures::synthetic(&root.join("synthetic-profile"), io.clone())
        });
        let (initial, telegram_history) = if let Some(harness) = &telegram_harness {
            let plan = harness.prepare(
                &crate::compatibility::fixtures::context("context"),
                vec![crate::compatibility::fixtures::fixture()["messages"][0]["ref"].clone()],
            );
            let before = harness.store.snapshot().unwrap();
            assert!(before.plans.contains(&plan));
            (
                harness.service.clone(),
                Some((harness.store.clone(), before)),
            )
        } else if let Some(harness) = &synthetic_harness {
            (harness.service.clone(), None)
        } else {
            (ProviderService::setup(), None)
        };
        let previous_context = initial.context();
        let mut runtime = crate::RuntimeState::with_archives(initial.clone(), owner);
        runtime.application_root = root.clone();
        let _drain = DrainArchive(&rt, runtime.archives.clone());
        let scope = rt.block_on(async {
            let path = root.join("synthetic.zip");
            fs::write(
                &path,
                crate::providers::discord::import_tests::package(1, "synthetic", false),
            )
            .unwrap();
            let outcome = runtime
                .discord_imports
                .start_or_retry(fs::File::open(path).unwrap())
                .await
                .unwrap()
                .wait()
                .await
                .unwrap();
            let sources = runtime
                .archives
                .open()
                .await
                .unwrap()
                .ready_sources()
                .await
                .unwrap();
            assert_eq!(sources.len(), 1);
            assert_eq!(outcome.checkpoint.scope, sources[0].source.scope());
            sources[0].source.scope()
        });
        let profile = format!("discord-{}", scope.account_id.as_uuid());
        let blocked = root.join(&profile);
        fs::write(&blocked, b"synthetic path obstruction; preserve me").unwrap();
        let app = commands_v2::register(tauri::test::mock_builder())
            .manage(Arc::new(runtime))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let request =
            json!({"contractVersion":2,"context":previous_context,"payload":{"scope":scope}});
        let start_blocked = || {
            synthetic_io.as_ref().map(|io| {
            let barrier = Arc::new(crate::compatibility::fixtures::PreflightBarrier::default());
            *io.search_barrier.lock().unwrap() = Some(barrier.clone());
            let webview = webview.clone();
            let search = json!({"contractVersion":2,"context":previous_context,"payload":{"query":"","conversations":[],"filters":null,"limit":500}});
            let (tx, rx) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || { tx.send(invoke(&webview, "search_messages_v2", search)).unwrap(); });
            rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(2), barrier.entered.notified()).await.unwrap(); });
            (barrier, rx, thread)
        })
        };
        let finish_blocked = |pending: Option<BlockedSearch>| {
            if let Some((barrier, rx, thread)) = pending {
                let result = rx.recv_timeout(std::time::Duration::from_secs(1));
                if result.is_err() {
                    barrier.release.notify_one();
                }
                thread.join().unwrap();
                assert_eq!(
                    result
                        .expect("source transition must wake pending search")
                        .unwrap_err()["code"],
                    "search_cancelled"
                );
            }
        };
        let pending = start_blocked();
        assert!(invoke(&webview, "select_archive_v2", request.clone()).is_err());
        finish_blocked(pending);
        if synthetic_io.is_some() {
            assert!(invoke(&webview, "search_messages_v2", json!({"contractVersion":2,"context":previous_context,"payload":{"query":"","conversations":[],"filters":null,"limit":500}})).is_ok());
        }
        assert_eq!(
            fs::read(&blocked).unwrap(),
            b"synthetic path obstruction; preserve me"
        );
        initial
            .check_optional(previous_context.as_ref())
            .expect("failed switch must not stop the old workspace");
        assert_eq!(initial.context(), previous_context);
        let bootstrap = invoke(
            &webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":previous_context,"payload":{}}),
        )
        .unwrap();
        assert_eq!(
            bootstrap["context"],
            serde_json::to_value(&previous_context).unwrap()
        );
        if let Some((store, before)) = &telegram_history {
            assert_eq!(&store.snapshot().unwrap(), before);
        }
        let sources = invoke(
            &webview,
            "list_sources_v2",
            json!({"contractVersion":2,"context":previous_context,"payload":{}}),
        )
        .unwrap();
        assert_eq!(sources["payload"].as_array().unwrap().len(), 1);
        // Remove only this test's synthetic obstruction and inject a test key; no Keychain access.
        fs::remove_file(&blocked).unwrap();
        let store = FoundationStore::open_with_test_key_and_payload_validator(
            blocked,
            StoreBinding {
                provider: scope.provider.clone(),
                profile,
            },
            [0x68; 32],
            Arc::new(crate::providers::discord::DiscordPayloadValidator),
        )
        .unwrap();
        let pending = start_blocked();
        let opened = invoke(&webview, "select_archive_v2", request).unwrap();
        finish_blocked(pending);
        assert_eq!(
            opened["context"]["scope"],
            serde_json::to_value(scope).unwrap()
        );
        assert_eq!(opened["payload"]["identity"]["state"], "ready");
        assert!(
            initial.check_optional(None).is_err(),
            "retire old service only after success"
        );
        assert_eq!(store.snapshot().unwrap().sources.len(), 1);
        let reopened = invoke(
            &webview,
            "select_archive_v2",
            json!({
                "contractVersion":2,"context":opened["context"],
                "payload":{"scope":opened["context"]["scope"]}
            }),
        )
        .unwrap();
        assert_eq!(reopened["context"]["scope"], opened["context"]["scope"]);
        assert_ne!(
            reopened["context"]["sessionGeneration"],
            opened["context"]["sessionGeneration"]
        );
        let stale_discord = invoke(
            &webview,
            "list_sources_v2",
            json!({"contractVersion":2,"context":opened["context"],"payload":{}}),
        )
        .unwrap_err();
        assert_eq!(stale_discord["code"], "stale_context");
        if let Some((store, before)) = &telegram_history {
            assert_eq!(&store.snapshot().unwrap(), before);
        }
        if previous_context.is_some() {
            let stale = invoke(
                &webview,
                "list_sources_v2",
                json!({"contractVersion":2,"context":previous_context,"payload":{}}),
            )
            .unwrap_err();
            assert_eq!(stale["code"], "scope_mismatch");
        }
    }
}

pub(crate) fn invoke(
    webview: &tauri::WebviewWindow<MockRuntime>,
    command: &str,
    request: Value,
) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        webview,
        tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(json!({"request": request})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    )
    .map(|body| body.deserialize().unwrap())
}

#[test]
fn compatibility_v2_setup_has_no_invented_identity_and_rejects_old_versions() {
    let webview = setup();
    let bootstrap = invoke(
        &webview,
        "get_bootstrap_snapshot_v2",
        json!({
            "contractVersion": 2, "context": null, "payload": {}
        }),
    )
    .unwrap();
    assert_eq!(bootstrap["contractVersion"], 2);
    assert_eq!(bootstrap["context"], Value::Null);
    assert_eq!(bootstrap["payload"]["identity"]["state"], "unavailable");
    for version in [0, 1, 3, 999] {
        let error = invoke(
            &webview,
            "get_bootstrap_snapshot_v2",
            json!({
                "contractVersion": version, "context": null, "payload": {}
            }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "unsupported_contract_version");
    }
}

#[test]
fn compatibility_v2_every_active_handler_requires_captured_context() {
    let webview = setup();
    for command in [
        "get_snapshot_v2",
        "search_messages_v2",
        "cancel_search_v2",
        "refresh_chats_v2",
        "prepare_selection_v2",
        "prepare_intent_v2",
        "authorize_plan_v2",
        "start_execution_v2",
        "get_jobs_v2",
        "cancel_job_v2",
    ] {
        let error = invoke(
            &webview,
            command,
            json!({
                "contractVersion": 2, "context": null, "payload": {}
            }),
        )
        .unwrap_err();
        assert_eq!(error["code"], "identity_unavailable", "{command}: {error}");
    }
}

#[test]
fn compatibility_v2_registration_has_no_legacy_identity_or_destructive_fallback() {
    let webview = setup();
    for command in [
        "get_snapshot",
        "get_bootstrap_snapshot",
        "search_messages",
        "refresh_chats",
        "prepare_selection",
        "prepare_own_messages",
        "prepare_chat_action",
        "prepare_sender_action",
        "authorize_plan",
        "start_execution",
        "get_jobs",
        "cancel_job",
    ] {
        let error = invoke(&webview, command, json!({"chatId": -1001})).unwrap_err();
        assert_eq!(error, Value::String(format!("Command {command} not found")));
    }
}

#[test]
fn compatibility_v2_rejects_version_scope_generation_and_malformed_refs_before_effects() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let reference = fixture()["messages"][0]["ref"].clone();
        let original =
            json!({"contractVersion":2,"context":active,"payload":{"messageRefs":[reference]}});
        let cases = [
            ("/contractVersion", json!(1), "unsupported_contract_version"),
            ("/context", Value::Null, "identity_unavailable"),
            (
                "/context/scope/provider",
                json!("synthetic"),
                "scope_mismatch",
            ),
            (
                "/context/scope/accountId",
                json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
                "scope_mismatch",
            ),
            (
                "/context/scope/sourceId",
                json!("22222222-2222-4222-8222-222222222222"),
                "scope_mismatch",
            ),
            (
                "/context/sessionGeneration",
                json!("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
                "stale_context",
            ),
            (
                "/context/sessionGeneration",
                json!("not-a-uuid"),
                "scope_mismatch",
            ),
            (
                "/payload/messageRefs/0/id",
                json!("00000000-0000-0000-0000-000000000000"),
                "scope_mismatch",
            ),
            (
                "/payload/messageRefs/0/resource/locatorVersion",
                json!(99),
                "scope_mismatch",
            ),
        ];
        for (path, value, code) in cases {
            let mut raw = original.clone();
            *raw.pointer_mut(path).unwrap() = value;
            let error = invoke(&harness.webview, "prepare_selection_v2", raw).unwrap_err();
            assert_eq!(error["code"], code, "{path}: {error}");
            assert!(gateway.operation_log().await.is_empty());
            assert!(harness.store.snapshot().unwrap().plans.is_empty());
        }
        let plan = harness.prepare(&active, vec![reference]);
        harness.authorize(&active, &plan);
        let mut stale = active.clone();
        stale.session_generation = uuid::Uuid::new_v4();
        for (command, payload) in [
            ("get_jobs_v2", json!({})),
            ("cancel_job_v2", json!({"jobId":uuid::Uuid::new_v4()})),
            (
                "authorize_plan_v2",
                json!({"planId":plan.id,"fingerprint":plan.fingerprint}),
            ),
            (
                "start_execution_v2",
                json!({"planId":plan.id,"fingerprint":plan.fingerprint,"irreversibleAcknowledged":true,"typedChatTitle":null}),
            ),
        ] {
            assert_eq!(
                harness.call(command, &stale, payload).unwrap_err()["code"],
                "stale_context"
            );
        }
        assert!(gateway.operation_log().await.is_empty());
        assert!(harness.store.snapshot().unwrap().jobs.is_empty());
    });
}

#[test]
fn compatibility_v2_owned_group_cleanup_and_all_filtered_search_fields_remain_available() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let conversation = fixture()["dirtyRefs"][0].clone();
        let before = gateway.chat_read_counts();
        let intents = harness
            .call(
                "get_intents_v2",
                &active,
                json!({"actionId":"catalog","targets":[conversation],"actor":null}),
            )
            .unwrap();
        assert!(
            intents
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["actionId"] == "delete_my_messages")
        );
        let own_intent = intents
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["actionId"] == "delete_my_messages")
            .unwrap();
        assert_eq!(own_intent["descriptors"][0]["kind"], "delete_remote_item");
        assert_eq!(own_intent["descriptors"][0]["confirmationTier"], "high");
        let clear_intent = intents
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["actionId"] == "clear_history")
            .unwrap();
        assert_eq!(clear_intent["descriptors"].as_array().unwrap().len(), 1);
        assert_eq!(clear_intent["descriptors"][0]["kind"], "clear_conversation");
        let own = harness
            .call(
                "prepare_intent_v2",
                &active,
                json!({"actionId":"delete_my_messages","targets":[conversation],"actor":null}),
            )
            .unwrap();
        assert_eq!(own["recipe"]["payload"]["operation"], "delete_my_messages");
        let filters = json!({"schema":"telegram.search_filters","version":1,"payload":{"chatKinds":["supergroup"],"contentKinds":["photo"],"direction":"others","minDate":"2020-01-01T00:00:00Z","maxDate":"2030-01-01T00:00:00Z","excludePinned":true,"privacyScan":false}});
        let page = harness
            .call(
                "search_messages_v2",
                &active,
                json!({"query":"","conversations":[conversation],"filters":filters,"limit":500}),
            )
            .unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        for record in page["items"].as_array().unwrap() {
            assert_eq!(record["kind"], "image");
            assert_eq!(record["resource"]["locatorPayload"]["messageId"], "13");
            assert_eq!(record["providerMetadata"]["payload"]["outgoing"], false);
            assert_eq!(record["providerMetadata"]["payload"]["pinned"], false);
        }
        assert_eq!(gateway.chat_read_counts().0, before.0);
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn final_review_own_message_intents_respect_known_kind_and_membership() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use crate::providers::telegram::native::ports::TelegramRead;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        gateway
            .set_chat_membership(-1003, cleaner_domain::ChatRole::Member, false)
            .await;
        for (chat_id, availability) in [
            (101, "unavailable"),
            (202, "unavailable"),
            (-1004, "unavailable"),
            (-1003, "unavailable"),
            (-1001, "live_preflight_required"),
        ] {
            let chat = gateway.chat_by_id(chat_id).await.unwrap().unwrap();
            let record =
                crate::providers::telegram::normalize::normalize_conversation(&active.scope, &chat)
                    .unwrap();
            let reference = retract_domain::ScopedResourceRef {
                scope: active.scope.clone(),
                id: *record.id.as_uuid(),
                resource: record.resource,
            };
            let intents = harness
                .call(
                    "get_intents_v2",
                    &active,
                    json!({"actionId":"catalog","targets":[reference],"actor":null}),
                )
                .unwrap();
            let own = intents
                .as_array()
                .unwrap()
                .iter()
                .find(|intent| intent["actionId"] == "delete_my_messages")
                .unwrap();
            assert_eq!(
                own["descriptors"][0]["availability"], availability,
                "chat {chat_id}"
            );
            if availability == "unavailable" {
                assert_eq!(
                    own["descriptors"][0]["unavailableReason"]["code"],
                    "permission_changed"
                );
            } else {
                assert_eq!(own["descriptors"][0]["requiresLivePreflight"], true);
            }
        }
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn compatibility_v2_blocked_foreign_history_does_not_lock_settings() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (first, _) = telegram(directory.path(), active.clone()).await;
        let plan = first.prepare(&active, vec![fixture()["messages"][0]["ref"].clone()]);
        let native =
            crate::providers::telegram::recipe::TelegramExecutionRecipe::validate_envelope(&plan)
                .unwrap();
        let job = crate::providers::telegram::normalize::normalize_job(
            &active.scope,
            &native,
            &crate::providers::telegram::model::JobRecord::new(&native),
            true,
        )
        .unwrap();
        first
            .store
            .transaction(|s| {
                s.jobs.push(job);
                Ok(())
            })
            .unwrap();
        drop(first);
        let other = context("otherContext");
        let (next, gateway) = telegram(directory.path(), other.clone()).await;
        assert_eq!(
            next.store.snapshot().unwrap().jobs[0].status,
            retract_domain::JobStatus::Blocked
        );
        assert!(!next.service.has_workers().await);
        // Valid shape, intentionally invalid settings: must reach settings
        // validation rather than reject because foreign historical jobs exist.
        let error=invoke(&next.webview,"save_connection_settings_v2",json!({"contractVersion":2,"context":other,"payload":{"tdlibPath":"","apiId":0,"apiHash":"synthetic-invalid","useTestDc":false}})).unwrap_err();
        assert_eq!(error["code"], "scope_mismatch");
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn compatibility_v2_errors_never_copy_private_native_content() {
    use crate::{error::AppError, providers::telegram::diagnostics::boundary_error};
    for error in [
        AppError::Gateway("private attachment secret.jpg".into()),
        AppError::SecureStore("/private/auth-token".into()),
        AppError::SystemAuthentication("password:123456".into()),
    ] {
        let wire = serde_json::to_value(boundary_error(error)).unwrap();
        assert_eq!(wire.as_object().unwrap().len(), 3);
        let text = wire.to_string();
        for secret in ["secret.jpg", "auth-token", "123456"] {
            assert!(!text.contains(secret));
        }
        serde_json::from_value::<retract_domain::SafeError>(wire).unwrap();
    }
}

#[test]
fn telegram_native_errors_keep_the_exact_v2_codes_at_the_provider_boundary() {
    use crate::{error::AppError, providers::telegram::diagnostics::boundary_error};

    for (error, expected) in [
        (
            AppError::Gateway("RETRACT_AMBIGUOUS_OUTCOME".into()),
            retract_domain::ErrorCode::AmbiguousOutcome,
        ),
        (
            AppError::Gateway("CHAT_ADMIN_REQUIRED".into()),
            retract_domain::ErrorCode::PermissionChanged,
        ),
        (
            AppError::Timeout("synthetic timeout".into()),
            retract_domain::ErrorCode::Transient,
        ),
        (
            AppError::SecureStore("synthetic storage failure".into()),
            retract_domain::ErrorCode::StatePersistenceFailed,
        ),
        (
            AppError::SystemAuthentication("synthetic auth failure".into()),
            retract_domain::ErrorCode::AuthenticationRequired,
        ),
    ] {
        assert_eq!(boundary_error(error).code, expected);
    }
}

#[test]
fn historical_v1_request_and_error_types_are_compatibility_only() {
    let request: crate::providers::telegram::model::AuthValueRequest =
        serde_json::from_value(json!({"value":"synthetic-code"})).unwrap();
    assert_eq!(request.value, "synthetic-code");
    let error = crate::error::CommandError::from(crate::error::AppError::NotFound);
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"code":"not_found","message":"the requested record was not found"})
    );
}

#[test]
fn compatibility_v2_bootstrap_discovers_verified_identity_but_mutations_require_context() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let response = invoke(
            &harness.webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":null,"payload":{}}),
        )
        .unwrap();
        assert_eq!(response["context"], serde_json::to_value(&active).unwrap());
        assert_eq!(response["payload"]["identity"]["state"], "ready");
        let mut stale = active.clone();
        stale.session_generation = uuid::Uuid::new_v4();
        assert_eq!(
            harness
                .call("get_bootstrap_snapshot_v2", &stale, json!({}))
                .unwrap_err()["code"],
            "stale_context"
        );
        for command in ["submit_auth_v2", "retry_identity_v2"] {
            assert_eq!(
                invoke(
                    &harness.webview,
                    command,
                    json!({"contractVersion":2,"context":null,"payload":{}})
                )
                .unwrap_err()["code"],
                "stale_context"
            );
        }
        assert_eq!(gateway.chat_read_counts().0, 0);
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn compatibility_v2_async_refresh_discards_old_context_results() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let harness = synthetic(directory.path(), io.clone());
        io.invalidate_refresh
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            harness
                .call(
                    "refresh_chats_v2",
                    &active,
                    json!({"conversations":[fixture()["messages"][2]["conversation"]]})
                )
                .unwrap_err()["code"],
            "stale_context"
        );
        assert_eq!(io.refreshes.load(std::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(io.catalog.load(std::sync::atomic::Ordering::Acquire), 0);
        assert!(io.calls.lock().unwrap().is_empty());
        assert!(harness.store.snapshot().unwrap().jobs.is_empty());
    });
}

#[test]
fn compatibility_v2_nonnumeric_recovery_requires_new_review_without_replay() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use std::sync::atomic::Ordering;
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        io.rate_limit_once.store(true, Ordering::Release);
        let original = synthetic(directory.path(), io.clone());
        let plan = original.prepare(&active, vec![fixture()["messages"][2]["ref"].clone()]);
        original.authorize(&active, &plan);
        let job = original.start(&active, &plan).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if original.store.snapshot().unwrap().jobs.iter().any(|j| {
                    j.id == job.id
                        && j.status == retract_domain::JobStatus::Queued
                        && j.retry_at.is_some()
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(io.calls.lock().unwrap().is_empty());
        // Capture real production-written bytes at the crash point. Teardown
        // stops the old worker, then replay restores these exact bytes: no test
        // creates/seals a plan, grant, job, progress cursor or retry timestamp.
        let checkpoint = std::fs::read(directory.path().join("jobs.enc")).unwrap();
        original.service.shutdown().await;
        let previous = Arc::downgrade(&original.store);
        drop(original);
        assert!(previous.upgrade().is_none());
        std::fs::write(directory.path().join("jobs.enc"), checkpoint).unwrap();
        *io.active.write().unwrap() = Some(active.clone());
        let recovered = synthetic(directory.path(), io.clone());
        invoke(
            &recovered.webview,
            "get_bootstrap_snapshot_v2",
            json!({"contractVersion":2,"context":active,"payload":{}}),
        )
        .unwrap();
        let finished = recovered.settled(&active, job.id).await;
        assert_eq!(finished.status, retract_domain::JobStatus::Failed);
        assert_eq!(finished.counters.deleted, 0);
        assert_eq!(finished.next_batch, 0);
        assert_eq!(finished.retry_at, None);
        assert!(
            finished
                .diagnostics
                .iter()
                .any(|error| { error.code == retract_domain::ErrorCode::RestartRequiresNewReview })
        );
        assert!(io.calls.lock().unwrap().is_empty());
        assert_eq!(io.catalog.load(Ordering::Acquire), 0);
        let state = encrypted_state(directory.path(), &active, KEY).unwrap();
        assert_eq!(state.jobs[0], finished);
    });
}

#[test]
fn review_round1_sender_actor_tag_cannot_change_the_reviewed_identity() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use crate::providers::telegram::locators::{TelegramActorKind, TelegramActorLocator};
        let directory = tempfile::tempdir().unwrap();
        let active = context("context");
        let (harness, gateway) = telegram(directory.path(), active.clone()).await;
        let chat = TelegramActorLocator::new(TelegramActorKind::Chat, "714")
            .unwrap()
            .resource(active.scope.account_id);
        let user = TelegramActorLocator::new(TelegramActorKind::User, "714")
            .unwrap()
            .resource(active.scope.account_id);
        let actor = |resource: retract_domain::ProviderResourceRef| json!({"id":resource.resource_id().unwrap(),"scope":active.scope,"resource":resource});
        let rejected=harness.call("prepare_intent_v2",&active,json!({"actionId":"delete_by_sender","targets":[fixture()["dirtyRefs"][0]],"actor":actor(chat)}));
        assert!(
            rejected.is_err(),
            "a Chat tag must never become a User sender plan: {rejected:?}"
        );
        assert!(harness.store.snapshot().unwrap().plans.is_empty());
        assert!(gateway.operation_log().await.is_empty());
        assert_eq!(gateway.chat_read_counts(), (0, 0));
        let plan=harness.call("prepare_intent_v2",&active,json!({"actionId":"delete_by_sender","targets":[fixture()["dirtyRefs"][0]],"actor":actor(user)})).unwrap();
        assert_eq!(
            plan["recipe"]["payload"]["actor"],
            json!({"kind":"user","nativeId":"714"})
        );
        assert_eq!(harness.store.snapshot().unwrap().plans.len(), 1);
        assert!(gateway.operation_log().await.is_empty());
        let supported_chat = TelegramActorLocator::new(TelegramActorKind::Chat, "-1002")
            .unwrap()
            .resource(active.scope.account_id);
        // The negative Chat encoding is supported, but this gateway fixture has
        // no message authored by that chat. It must resolve as missing, not be
        // relabeled as a User or manufacture another executable plan.
        let missing=harness.call("prepare_intent_v2",&active,json!({"actionId":"delete_by_sender","targets":[fixture()["dirtyRefs"][0]],"actor":actor(supported_chat)})).unwrap_err();
        assert_eq!(missing["code"], "not_found");
        assert_eq!(harness.store.snapshot().unwrap().plans.len(), 1);
        assert!(gateway.operation_log().await.is_empty());
    });
}

#[test]
fn review_round1_intent_catalog_rejects_malformed_provider_output() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use crate::providers::ports::IntentDescriptor;
        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let harness = synthetic(directory.path(), io.clone());
        let request =
            json!({"actionId":"catalog","targets":[fixture()["messages"][2]["ref"]],"actor":null});
        let valid: Vec<IntentDescriptor> = serde_json::from_value(
            harness
                .call("get_intents_v2", &active, request.clone())
                .unwrap(),
        )
        .unwrap();
        let mut cases = vec![];
        let mut bad = valid.clone();
        bad[0].action_id = String::new();
        cases.push(bad);
        let mut bad = valid.clone();
        bad.push(bad[0].clone());
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].label = "bad\nlabel".into();
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].descriptors.clear();
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].descriptors[0].batch.max_targets = 0;
        cases.push(bad);
        let mut bad = valid.clone();
        bad[0].descriptors[0].availability = retract_domain::Availability::Unavailable;
        cases.push(bad);
        for (index, bad) in cases.into_iter().enumerate() {
            *io.intent_override.lock().unwrap() = Some(bad);
            assert_eq!(
                harness
                    .call("get_intents_v2", &active, request.clone())
                    .unwrap_err()["code"],
                "unsupported_schema",
                "case {index}"
            );
            assert!(harness.store.snapshot().unwrap().plans.is_empty());
            assert!(io.calls.lock().unwrap().is_empty());
        }
    });
}

#[test]
fn review_round1_retry_preserves_live_deadlines_but_recovery_requires_new_review() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;
        use std::sync::atomic::Ordering;
        for recover in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let active = context("syntheticContext");
            let io = Arc::new(SyntheticIo::new(active.clone()));
            let barrier = Arc::new(PreflightBarrier::default());
            *io.preflight_barrier.lock().unwrap() = Some(barrier.clone());
            io.rate_limit_once.store(true, Ordering::Release);
            io.preflight_delay_ms.store(150, Ordering::Release);
            let mut harness = synthetic(directory.path(), io.clone());
            let plan = harness.prepare(&active, vec![fixture()["messages"][2]["ref"].clone()]);
            harness.authorize(&active, &plan);
            let job = harness.start(&active, &plan).unwrap();
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                barrier.entered.notified(),
            )
            .await
            .unwrap();
            // Start the native deadline at the controlled preflight boundary, not
            // before potentially slow harness/authorization setup. The original
            // 150ms response latency and absolute-deadline assertions stay intact.
            let deadline = chrono::Utc::now() + chrono::Duration::milliseconds(1650);
            *io.rate_deadline.lock().unwrap() = Some(deadline);
            barrier.release.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if harness.store.snapshot().unwrap().jobs[0].retry_at.is_some() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                harness.store.snapshot().unwrap().jobs[0].retry_at,
                Some(deadline),
                "the native absolute deadline must not be rounded down or rebased"
            );
            if recover {
                let checkpoint = std::fs::read(directory.path().join("jobs.enc")).unwrap();
                harness.service.shutdown().await;
                let old = Arc::downgrade(&harness.store);
                drop(harness);
                assert!(old.upgrade().is_none());
                std::fs::write(directory.path().join("jobs.enc"), checkpoint).unwrap();
                *io.active.write().unwrap() = Some(active.clone());
                harness = synthetic(directory.path(), io.clone());
                invoke(
                    &harness.webview,
                    "get_bootstrap_snapshot_v2",
                    json!({"contractVersion":2,"context":active,"payload":{}}),
                )
                .unwrap();

                let stopped = harness.settled(&active, job.id).await;
                assert_eq!(stopped.status, retract_domain::JobStatus::Failed);
                assert_eq!(stopped.counters.deleted, 0);
                assert_eq!(stopped.retry_at, None);
                assert!(stopped.diagnostics.iter().any(|error| {
                    error.code == retract_domain::ErrorCode::RestartRequiresNewReview
                }));
                assert_eq!(io.preflights.lock().unwrap().len(), 1);
                assert!(io.calls.lock().unwrap().is_empty());
                continue;
            }
            while chrono::Utc::now() + chrono::Duration::milliseconds(30) < deadline {
                assert_eq!(io.preflights.lock().unwrap().len(), 1);
                assert!(io.calls.lock().unwrap().is_empty());
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let finished = harness.settled(&active, job.id).await;
            assert_eq!(finished.counters.deleted, 1);
            let preflights = io.preflights.lock().unwrap();
            assert_eq!(preflights.len(), 2);
            assert!(preflights[1] >= deadline);
            assert_eq!(io.calls.lock().unwrap().len(), 1);
        }
    });
}

#[test]
fn cancellation_waits_for_an_in_flight_mutation_to_acknowledge_and_quiesce() {
    tauri::async_runtime::block_on(async {
        use super::fixtures::*;

        let directory = tempfile::tempdir().unwrap();
        let active = context("syntheticContext");
        let io = Arc::new(SyntheticIo::new(active.clone()));
        let barrier = Arc::new(PreflightBarrier::default());
        *io.mutation_barrier.lock().unwrap() = Some(barrier.clone());
        let harness = synthetic(directory.path(), io.clone());
        let plan = harness.prepare(&active, vec![fixture()["messages"][2]["ref"].clone()]);
        harness.authorize(&active, &plan);
        let job = harness.start(&active, &plan).unwrap();

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            barrier.entered.notified(),
        )
        .await
        .unwrap();
        let cancellation = {
            let service = harness.service.clone();
            let active = active.clone();
            tokio::spawn(async move {
                service
                    .active(
                        "cancel",
                        json!({
                            "contractVersion": 2,
                            "context": active,
                            "payload": {"jobId": job.id}
                        }),
                    )
                    .await
                    .unwrap()
            })
        };
        tokio::task::yield_now().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), async {
                while !cancellation.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_err()
        );
        assert_eq!(
            harness.store.snapshot().unwrap().jobs[0].status,
            retract_domain::JobStatus::Cancelled,
            "cancellation must be durable before waiting for an in-flight mutation"
        );
        assert!(io.calls.lock().unwrap().is_empty());

        barrier.release.notify_one();
        let cancelled = tokio::time::timeout(std::time::Duration::from_secs(2), cancellation)
            .await
            .unwrap()
            .unwrap();
        let cancelled: retract_domain::ScopedJobRecord =
            serde_json::from_value(cancelled["payload"].clone()).unwrap();
        assert_eq!(cancelled.status, retract_domain::JobStatus::Cancelled);
        assert_eq!(io.calls.lock().unwrap().len(), 1);
        let settled = harness.settled(&active, job.id).await;
        assert_eq!(settled.status, retract_domain::JobStatus::Cancelled);
    });
}
