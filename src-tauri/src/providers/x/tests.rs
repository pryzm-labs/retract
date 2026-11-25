use super::*;
use crate::persistence::archive::{
    ArchiveKey, ArchiveOwner, ArchiveSearch, ArchiveStore, NewArchiveImport,
};
use sha2::{Digest, Sha256};
use std::sync::{Arc, atomic::AtomicBool};

const ARCHIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test-fixtures/x-archive/authored.zip"
);

#[test]
fn authored_archive_import_search_and_replay() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().canonicalize().unwrap().join("store.db");
            let open_owner = || {
                let path = path.clone();
                Arc::new(ArchiveOwner::with_opener(move || {
                ArchiveStore::open(
                    path.clone(),
                    ArchiveKey::new([0x23; 32]),
                    crate::providers::archive::application_validators(),
                )
                }))
            };
            let owner = open_owner();
            let run = || {
                let owner = owner.clone();
                tokio::task::spawn_blocking(move || {
                    let file = std::fs::File::open(ARCHIVE).unwrap();
                    import_file(file, &owner, &AtomicBool::new(false))
                })
            };
            let service = owner.open().await.unwrap();
            let digest = Sha256::digest(std::fs::read(ARCHIVE).unwrap());
            let legacy = service.resolve_or_register_import(NewArchiveImport {
                provider: key(), native_identity: user("9007199254740993"),
                display_name: "fixture".into(), username: Some("fixture".into()), avatar: None,
                fingerprint: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
                schema_profile: profile(), parser_policy: "x.coordinator.v1".into(),
                observed_at: chrono::Utc::now(),
            }).await.unwrap();
            service.finish_import(&legacy.session.unwrap()).await.unwrap();
            let scope = run().await.unwrap().unwrap();
            assert_ne!(scope.source_id, legacy.source.id);
            assert_eq!(run().await.unwrap().unwrap(), scope);
            owner.shutdown().await;
            drop(service);
            let owner = open_owner();
            let service = owner.open().await.unwrap();
            let results = service
                .search(&ArchiveSearch {
                    scope:scope.clone(),
                    text: String::new(),
                    kinds: vec![],
                    conversations: vec![],
                    privacy_only: false,
                    author: None,
                    before: None,
                    after: None,
                    cursor: None,
                    limit: 10,
                })
                .await
                .unwrap();
            assert_eq!(results.items.len(), 2);
            assert!(results.items.iter().any(|post| post.searchable_text.starts_with("RT @fixture")));
            let post = results.items.iter().find(|post| post.resource.canonical_key == "9007199254740995").unwrap();
            assert_eq!(post.resource.canonical_key, "9007199254740995");
            assert!(post.searchable_text.contains("synthetic authored"));
            assert_eq!(post.evidence, retract_domain::EvidenceState::Archive);
            assert!(!post.privacy_findings.is_empty());
            let entries = service.ready_sources().await.unwrap();
            assert_eq!(entries.len(), 2);
            let entry = entries.into_iter().find(|entry| entry.source.id == scope.source_id).unwrap();
            let connection=build_connection(entry,service).unwrap();
            let registration=connection.registration().await.unwrap();
            assert!(registration.reviewed_lifecycle().is_err());
            assert!(registration.live_connection().is_err());
            assert!(registration.remediation().is_err());
            let backend=crate::provider_service::ProviderService::new(connection);
            let ctx=backend.context().unwrap();
            let call=|payload:serde_json::Value|serde_json::json!({"contractVersion":2,"context":ctx,"payload":payload});
            let snapshot=backend.bootstrap(call(serde_json::json!({}))).await.unwrap();
            assert_eq!(snapshot["payload"]["auth"]["schema"],"x.account");
            let query=call(serde_json::json!({"query":"synthetic","limit":10,"conversations":[],"filters":{"schema":"x.search_filters","version":1,"payload":{"privacyScan":true}}}));
            let result=backend.run_search(backend.prepare_search(query).unwrap()).await.unwrap();
            assert_eq!(result["payload"]["items"].as_array().unwrap().len(),1);
            assert!(backend.active("prepare_intent",call(serde_json::json!({"actionId":"delete","targets":[],"actor":null}))).await.is_err());
            owner.shutdown().await;
        });
}

use crate::providers::archive_file::zip_fixture as fixture;
#[test]
fn repost_classification_uses_structured_evidence() {
    use discord_archive::{ArchiveInventory, ArchiveLimits};
    let mut rows = vec![];
    for (id, evidence) in [
        ("1", serde_json::json!({})),
        ("2", serde_json::json!({"retweeted": true})),
        ("3", serde_json::json!({"retweeted_status":{"id_str":"4"}})),
    ] {
        let mut tweet = serde_json::json!({"id_str":id, "full_text":"RT @fixture is the text of my example: demo@example.org", "created_at":"Fri Jan 02 03:04:05 +0000 2026"});
        tweet
            .as_object_mut()
            .unwrap()
            .extend(evidence.as_object().unwrap().clone());
        rows.push(serde_json::json!({"tweet":tweet}));
    }
    let tweets = format!("window.YTD.tweets.part0 = {}", serde_json::json!(rows));
    let zip = fixture::zip(&[
        (
            "data/account.js",
            br#"window.YTD.account.part0 = [{"account":{"accountId":"123","username":"fixture"}}]"#,
        ),
        ("data/tweets.js", tweets.as_bytes()),
    ]);
    let mut inventory = ArchiveInventory::inspect(
        std::io::Cursor::new(zip),
        ArchiveLimits::default(),
        &fixture::NeverCancel,
    )
    .unwrap();
    let (_, posts) = parser::parse(&mut inventory).unwrap();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id_str, "1");
}

#[test]
fn rejects_executable_trailing_script_multipart_and_wrong_profile() {
    use discord_archive::{ArchiveInventory, ArchiveLimits};
    const A: &[u8] =
        br#"window.YTD.account.part0 = [{"account":{"accountId":"123","username":"fixture"}}]"#;
    let rejects = |entries: &[(&str, &[u8])]| {
        let input = std::io::Cursor::new(fixture::zip(entries));
        let mut inventory =
            ArchiveInventory::inspect(input, ArchiveLimits::default(), &fixture::NeverCancel)
                .unwrap();
        assert!(parser::parse(&mut inventory).is_err());
    };
    for malformed in [
        "window.YTD.tweets.part0 = [];globalThis.mutate()",
        "window.YTD.tweets.part1 = []",
        "window.YTD.tweets.part0 = function(){}",
        r#"window.YTD.tweets.part0 = [{"tweet":{"id_str":"123","id":"456","full_text":"fixture","created_at":"invalid"}}]"#,
    ] {
        rejects(&[
            ("data/account.js", A),
            ("data/tweets.js", malformed.as_bytes()),
        ]);
    }
    rejects(&[("data/account.js", A), ("data/tweet.js", b"[]")]);
    rejects(&[
        ("data/account.js", A),
        ("data/tweets.js", b"window.YTD.tweets.part0 = []"),
        ("data/tweets-part1.js", b"[]"),
    ]);
    let row = r#"{"tweet":{"id_str":"123","full_text":"fixture","created_at":"Fri Jan 02 03:04:05 +0000 2026"}}"#;
    let rows = format!("window.YTD.tweets.part0 = [{}]", vec![row; 4097].join(","));
    rejects(&[("data/account.js", A), ("data/tweets.js", rows.as_bytes())]);
}
