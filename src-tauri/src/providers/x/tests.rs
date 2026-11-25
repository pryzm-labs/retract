use super::*;
use crate::persistence::archive::{ArchiveKey, ArchiveOwner, ArchiveSearch, ArchiveStore};
use std::sync::{Arc, atomic::AtomicBool};

#[test]
fn authored_archive_import_search_and_replay() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().canonicalize().unwrap().join("store.db");
            let owner = Arc::new(ArchiveOwner::with_opener(move || {
                ArchiveStore::open(
                    path.clone(),
                    ArchiveKey::new([0x23; 32]),
                    crate::providers::archive::application_validators(),
                )
            }));
            let run = || {
                let owner = owner.clone();
                tokio::task::spawn_blocking(move || {
                    let file = std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/test-fixtures/x-archive/authored.zip")).unwrap();
                    import_file(file, &owner, &AtomicBool::new(false))
                })
            };
            let scope = run().await.unwrap().unwrap();
            assert_eq!(run().await.unwrap().unwrap(), scope);
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
            assert_eq!(results.items.len(), 1);
            let post = &results.items[0];
            assert_eq!(post.resource.canonical_key, "9007199254740995");
            assert!(post.searchable_text.contains("synthetic authored"));
            assert_eq!(post.evidence, retract_domain::EvidenceState::Archive);
            assert!(!post.privacy_findings.is_empty());
            let entry=service.ready_sources().await.unwrap().pop().unwrap();
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
