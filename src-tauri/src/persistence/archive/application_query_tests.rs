//! Real encrypted synthetic Discord stores, never a live export or account.
use super::worker_tests::{Gate, Release, runtime};
use super::*;
use crate::providers::{
    discord::{
        DiscordPayloadValidator,
        application::DiscordApplicationQuery,
        import::DiscordImportOwner,
        import_tests::{ImportTestLaunch, zip_fixture},
        locators::discord_provider_key,
    },
    ports::ApplicationQuery,
};
use retract_domain::{ActiveContext, ScopedResourceRef};
use std::sync::{Arc, atomic::Ordering};

fn validators() -> std::collections::BTreeMap<
    retract_domain::ProviderKey,
    Arc<dyn crate::persistence::ProviderPayloadValidator>,
> {
    std::collections::BTreeMap::from([(
        discord_provider_key(),
        Arc::new(DiscordPayloadValidator) as Arc<_>,
    )])
}

async fn prepared(
    gate: Arc<Gate>,
    poison_unrelated: bool,
) -> (tempfile::TempDir, Arc<ArchiveService>, ActiveContext) {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let path = root.join("archive.db");
    let database = path.clone();
    let archives = Arc::new(ArchiveOwner::with_opener(move || {
        ArchiveStore::open(database.clone(), ArchiveKey::new([0x85; 32]), validators())
    }));
    let messages = (0..205).map(|i| format!(r#"{{"ID":{},"Timestamp":"2030-01-02 03:04:05","Contents":"alpha outside@example.test","Attachments":""}}"#, 1985931830091579392_u64 + i)).collect::<Vec<_>>().join(",");
    let messages = format!("[{messages}]");
    let bytes = zip_fixture::zip(&[
        ("Account/user.json", br#"{"id":"9007199254741001","username":"invented_owner"}"#),
        ("Messages/index.json", b"{}"),
        ("Messages/c9007199254741101/channel.json", br#"{"id":"9007199254741101","type":"DM"}"#),
        ("Messages/c9007199254741101/messages.json", messages.as_bytes()),
        ("Messages/c9007199254741102/channel.json", br#"{"id":"9007199254741102","type":"DM"}"#),
        ("Messages/c9007199254741102/messages.json", br#"[{"ID":1985569442225979392,"Timestamp":"2030-01-01 03:04:05","Contents":"alpha rare@example.test","Attachments":""}]"#),
    ]);
    let selected = root.join("synthetic.zip");
    std::fs::write(&selected, bytes).unwrap();
    let imports = DiscordImportOwner::new(archives.clone());
    let outcome = imports
        .start_for_test(std::fs::File::open(selected).unwrap())
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    archives.shutdown().await;
    let context = ActiveContext {
        scope: outcome.checkpoint.scope,
        session_generation: uuid::Uuid::new_v4(),
    };
    let service = ArchiveService::open(move || {
        let mut store = ArchiveStore::open(path, ArchiveKey::new([0x85;32]), validators())?;
        if poison_unrelated {
            store.transaction(|tx| {
                tx.execute("UPDATE conversation_observations SET record_json='{}' WHERE json_extract(record_json, '$.resource.canonicalKey')='9007199254741101'", []).unwrap();
                Ok(())
            })?;
        }
        store.before_commit = Some(Box::new(move || gate.wait()));
        Ok(store)
    }).await.unwrap();
    (directory, service, context)
}

fn request(limit: u32) -> crate::compatibility::model_v2::SearchRequest {
    crate::compatibility::model_v2::SearchRequest {
        query: "alpha".into(),
        conversations: vec![],
        filters: None,
        limit,
    }
}

fn reference(record: &retract_domain::ConversationRecord) -> ScopedResourceRef {
    ScopedResourceRef {
        id: *record.id.as_uuid(),
        scope: record.scope.clone(),
        resource: record.resource.clone(),
    }
}

#[test]
fn focused_discord_limit_continuation_and_selected_rare_privacy_match() {
    runtime().block_on(async {
        let (_directory, service, context) = prepared(Arc::new(Gate::default()), false).await;
        let query =
            DiscordApplicationQuery::new(context.clone(), "9007199254741001", service.clone())
                .unwrap();
        let page = query.search_filtered(&context, request(203)).await.unwrap();
        assert_eq!(page.items.len(), 203);
        assert!(
            page.next_cursor.is_some(),
            "known additional matches require continuation"
        );
        let rest = service
            .search(&ArchiveSearch {
                scope: context.scope.clone(),
                text: "alpha".into(),
                kinds: vec![],
                conversations: vec![],
                privacy_only: false,
                author: Some(page.items[0].author_id),
                before: None,
                after: None,
                cursor: page.next_cursor,
                limit: 200,
            })
            .await
            .unwrap();
        assert_eq!(rest.items.len(), 3);
        assert!(
            rest.items
                .iter()
                .all(|item| page.items.iter().all(|seen| item.id != seen.id))
        );
        let rooms = query.conversations(&context).await.unwrap();
        let selected = rooms
            .iter()
            .find(|room| room.resource.canonical_key == "9007199254741102")
            .unwrap();
        let mut scoped = request(1);
        scoped.conversations = vec![reference(selected)];
        scoped.filters = Some(retract_domain::VersionedPayload {
            schema: "discord.search_filters".into(),
            version: 1,
            payload: serde_json::json!({"privacyScan": true}),
        });
        let found = query.search_filtered(&context, scoped).await.unwrap();
        assert_eq!(found.items.len(), 1);
        assert!(found.items[0].searchable_text.contains("rare@"));
        assert!(found.next_cursor.is_none());
        service.shutdown().await;
    });
}

#[test]
fn focused_discord_superseding_others_cancels_a_queued_read_explicitly() {
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        let _release = Release(gate.clone());
        let (_directory, service, context) = prepared(gate.clone(), false).await;
        let query =
            DiscordApplicationQuery::new(context.clone(), "9007199254741001", service.clone())
                .unwrap();
        gate.armed.store(true, Ordering::Release);
        let block_request = super::worker_tests::search(context.scope.clone(), "alpha");
        let mut blocker = Box::pin(service.search(&block_request));
        assert!(futures_util::poll!(blocker.as_mut()).is_pending());
        gate.entered().await;
        let mut stale = Box::pin(query.search_filtered(&context, request(1)));
        assert!(futures_util::poll!(stale.as_mut()).is_pending());
        let mut newer = request(1);
        newer.filters = Some(retract_domain::VersionedPayload {
            schema: "discord.search_filters".into(),
            version: 1,
            payload: serde_json::json!({"direction": "others"}),
        });
        assert!(
            query
                .search_filtered(&context, newer)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        // Must wake while the worker is still blocked, closing the queued receiver.
        assert!(
            matches!(
                futures_util::poll!(stale.as_mut()),
                std::task::Poll::Ready(Err(_))
            ),
            "superseded search must fail explicitly without waiting for worker"
        );
        gate.release();
        blocker.await.unwrap();
        service.shutdown().await;
    });
}

#[test]
fn focused_discord_new_search_cancels_prior_read_and_returns_its_own_results() {
    runtime().block_on(async {
        let gate = Arc::new(Gate::default());
        let _release = Release(gate.clone());
        let (_directory, service, context) = prepared(gate.clone(), false).await;
        let query =
            DiscordApplicationQuery::new(context.clone(), "9007199254741001", service.clone())
                .unwrap();
        gate.armed.store(true, Ordering::Release);
        let block_request = super::worker_tests::search(context.scope.clone(), "alpha");
        let mut blocker = Box::pin(service.search(&block_request));
        assert!(futures_util::poll!(blocker.as_mut()).is_pending());
        gate.entered().await;
        let mut stale = Box::pin(query.search_filtered(&context, request(1)));
        assert!(futures_util::poll!(stale.as_mut()).is_pending());
        let mut newer_request = request(1);
        newer_request.query = "rare".into();
        let mut newer = Box::pin(query.search_filtered(&context, newer_request));
        assert!(futures_util::poll!(newer.as_mut()).is_pending());
        assert!(matches!(
            futures_util::poll!(stale.as_mut()),
            std::task::Poll::Ready(Err(_))
        ));
        gate.release();
        blocker.await.unwrap();
        let page = newer.await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0].searchable_text.contains("rare@"));
        service.shutdown().await;
    });
}

#[test]
fn focused_discord_refresh_direct_lookup_ignores_unrelated_invalid_records() {
    runtime().block_on(async {
        let (_directory, service, context) = prepared(Arc::new(Gate::default()), true).await;
        let query =
            DiscordApplicationQuery::new(context.clone(), "9007199254741001", service.clone())
                .unwrap();
        let resource =
            crate::providers::discord::locators::DiscordChannelLocator::new("9007199254741102")
                .unwrap()
                .resource(context.scope.account_id);
        let target = ScopedResourceRef {
            id: resource.resource_id().unwrap(),
            scope: context.scope.clone(),
            resource,
        };
        let found = query.refresh(&context, vec![target.clone()]).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].resource, target.resource);
        let mut many = vec![target.clone(), target.clone()];
        for channel in 9007199254741200_u64..9007199254741401 {
            let resource = crate::providers::discord::locators::DiscordChannelLocator::new(
                &channel.to_string(),
            )
            .unwrap()
            .resource(context.scope.account_id);
            many.push(ScopedResourceRef {
                id: resource.resource_id().unwrap(),
                scope: context.scope.clone(),
                resource,
            });
        }
        assert_eq!(query.refresh(&context, many).await.unwrap(), found);
        let mut foreign = target.clone();
        foreign.scope.source_id = uuid::Uuid::new_v4().try_into().unwrap();
        assert!(query.refresh(&context, vec![foreign]).await.is_err());
        let mut forged = target;
        forged.resource.locator_payload = serde_json::json!({"channelId": "9007199254741101"});
        assert!(query.refresh(&context, vec![forged]).await.is_err());
        assert!(query.refresh(&context, vec![]).await.unwrap().is_empty());
        service.shutdown().await;
    });
}
