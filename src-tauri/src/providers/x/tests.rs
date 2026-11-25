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
                schema_profile: profile(false), parser_policy: "x.coordinator.v1".into(),
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
fn multipart_fixture_reopens_with_posts_from_every_part() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().canonicalize().unwrap().join("multipart.db");
    let open = || {
        let db = db.clone();
        Arc::new(ArchiveOwner::with_opener(move || {
            ArchiveStore::open(
                db.clone(),
                ArchiveKey::new([0x47; 32]),
                crate::providers::archive::application_validators(),
            )
        }))
    };
    tauri::async_runtime::block_on(async {
        let owner = open();
        let run = || {
            let owner = owner.clone();
            tokio::task::spawn_blocking(move || {
                let file = std::fs::File::open(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/test-fixtures/x-archive/multipart.zip"
                ))
                .unwrap();
                import_file(file, &owner, &AtomicBool::new(false))
            })
        };
        let scope = run().await.unwrap().unwrap();
        assert_eq!(run().await.unwrap().unwrap(), scope);
        owner.shutdown().await;
        let reopened = open();
        let service = reopened.open().await.unwrap();
        let entries = service.ready_sources().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source.schema_profile.schema, "x.ytd.multipart");
        let mut query = ArchiveSearch {
            scope,
            text: "fixture".into(),
            kinds: vec![],
            conversations: vec![],
            privacy_only: false,
            author: None,
            before: None,
            after: None,
            cursor: None,
            limit: 10,
        };
        let mut ids = service
            .search(&query)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|post| post.resource.canonical_key)
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(
            ids,
            ["9007199254740997", "9007199254740998", "9007199254740999"]
        );
        query.privacy_only = true;
        let matches = service.search(&query).await.unwrap();
        assert_eq!(matches.items.len(), 1);
        assert_eq!(matches.items[0].resource.canonical_key, "9007199254740999");
        reopened.shutdown().await;
    });
}

fn read_posts<R: std::io::Read + std::io::Seek>(
    inventory: &mut discord_archive::ArchiveInventory<R>,
) -> Result<Vec<parser::Post>, discord_archive::ArchiveError> {
    let layout = parser::inspect(inventory)?;
    let mut posts = Vec::new();
    parser::visit_posts(inventory, &layout.parts, |post| {
        posts.push(post);
        Ok(())
    })?;
    Ok(posts)
}

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
    let posts = read_posts(&mut inventory).unwrap();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id_str, "1");
}

#[test]
fn rejects_executable_trailing_script_and_wrong_profile() {
    use discord_archive::{ArchiveInventory, ArchiveLimits};
    const A: &[u8] =
        br#"window.YTD.account.part0 = [{"account":{"accountId":"123","username":"fixture"}}]"#;
    let rejects = |entries: &[(&str, &[u8])]| {
        let input = std::io::Cursor::new(fixture::zip(entries));
        let mut inventory =
            ArchiveInventory::inspect(input, ArchiveLimits::default(), &fixture::NeverCancel)
                .unwrap();
        assert!(read_posts(&mut inventory).is_err());
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

const ACCOUNT_JS: &[u8] =
    br#"window.YTD.account.part0 = [{"account":{"accountId":"101","username":"fixture_owner"}}]"#;

fn synthetic_part(number: usize, rows: &[serde_json::Value]) -> String {
    format!(
        "window.YTD.tweets.part{number} = {}",
        serde_json::json!(rows)
    )
}

fn synthetic_post(id: usize, text: &str) -> serde_json::Value {
    serde_json::json!({"tweet":{"id_str":id.to_string(),"created_at":"Fri Jan 02 03:04:05 +0000 2026","full_text":text}})
}

#[test]
fn multipart_enforces_contiguous_names_matching_wrappers_and_json_framing() {
    let empty = synthetic_part(0, &[]);
    for (name, body) in [
        ("data/tweets-part0.js", "window.YTD.tweets.part0 = []"),
        ("data/tweets-part01.js", "window.YTD.tweets.part1 = []"),
        ("data/tweets-part2.js", "window.YTD.tweets.part2 = []"),
        ("data/tweets-part32.js", "window.YTD.tweets.part32 = []"),
        ("data/tweets-part1.js", "window.YTD.tweets.part0 = []"),
        ("data/tweets-part1.js", "window.YTD.tweets.part1 = [] extra"),
        ("data/tweets-part1.js", "window.YTD.tweets.part1 = ["),
        ("data/tweets-part1.js", "window.YTD.tweets.part1 = [,]"),
    ] {
        let zip = fixture::zip(&[
            ("data/account.js", ACCOUNT_JS),
            ("data/tweets.js", empty.as_bytes()),
            (name, body.as_bytes()),
        ]);
        let mut inventory = discord_archive::ArchiveInventory::inspect(
            std::io::Cursor::new(zip),
            Default::default(),
            &fixture::NeverCancel,
        )
        .unwrap();
        assert!(read_posts(&mut inventory).is_err(), "{name} {body}");
    }
}

#[test]
fn multipart_visits_two_digit_parts_numerically_and_stops_when_cancelled() {
    struct Cancel(AtomicBool);
    impl discord_archive::Cancellation for Cancel {
        fn is_cancelled(&self) -> bool {
            self.0.load(std::sync::atomic::Ordering::Relaxed)
        }
    }
    let mut entries = vec![("data/account.js".to_owned(), ACCOUNT_JS.to_vec())];
    for n in (0..12).rev() {
        let name = if n == 0 {
            "data/tweets.js".into()
        } else {
            format!("data/tweets-part{n}.js")
        };
        entries.push((
            name,
            synthetic_part(n, &[synthetic_post(n + 1, "fixture")]).into_bytes(),
        ));
    }
    let zip = fixture::zip(
        &entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect::<Vec<_>>(),
    );
    let cancel = Cancel(AtomicBool::new(false));
    let mut inventory = discord_archive::ArchiveInventory::inspect(
        std::io::Cursor::new(zip),
        Default::default(),
        &cancel,
    )
    .unwrap();
    let layout = parser::inspect(&mut inventory).unwrap();
    let mut visited = Vec::new();
    let result = parser::visit_posts(&mut inventory, &layout.parts, |post| {
        visited.push(post.id_str);
        if visited.len() == 11 {
            cancel.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    });
    assert_eq!(result, Err(discord_archive::ArchiveError::Cancelled));
    assert_eq!(visited, (1..=11).map(|n| n.to_string()).collect::<Vec<_>>());
}

#[test]
fn multipart_accepts_parts_larger_than_the_single_part_byte_and_row_limits() {
    let row = synthetic_post(123, &"x".repeat(512)).to_string();
    let large = format!("window.YTD.tweets.part0 = [{}]", vec![row; 4097].join(","));
    assert!(large.len() > 2 * 1024 * 1024);
    let zip = fixture::zip(&[
        ("data/account.js", ACCOUNT_JS),
        ("data/tweets.js", large.as_bytes()),
        ("data/tweets-part1.js", b"window.YTD.tweets.part1 = []"),
    ]);
    let mut inventory = discord_archive::ArchiveInventory::inspect(
        std::io::Cursor::new(zip),
        Default::default(),
        &fixture::NeverCancel,
    )
    .unwrap();
    assert_eq!(read_posts(&mut inventory).unwrap().len(), 4097);
}

#[test]
fn multipart_late_failures_keep_staged_posts_out_of_ready_sources() {
    tauri::async_runtime::block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().canonicalize().unwrap().join("db");
        let owner = Arc::new(ArchiveOwner::with_opener(move || {
            ArchiveStore::open(
                db.clone(),
                ArchiveKey::new([0x35; 32]),
                crate::providers::archive::application_validators(),
            )
        }));
        let first = synthetic_part(
            0,
            &(1..=16)
                .map(|id| synthetic_post(id, "fixture"))
                .collect::<Vec<_>>(),
        );
        for last in [
            synthetic_part(1, &[synthetic_post(1, "conflicting text")]),
            "window.YTD.tweets.part1 = [".into(),
        ] {
            let zip = fixture::zip(&[
                ("data/account.js", ACCOUNT_JS),
                ("data/tweets.js", first.as_bytes()),
                ("data/tweets-part1.js", last.as_bytes()),
            ]);
            let path = temp.path().join("selected.zip");
            std::fs::write(&path, zip).unwrap();
            let importing = owner.clone();
            let result = tokio::task::spawn_blocking(move || {
                import_file(
                    std::fs::File::open(path).unwrap(),
                    &importing,
                    &AtomicBool::new(false),
                )
            })
            .await
            .unwrap();
            assert!(matches!(
                result,
                Err(
                    crate::persistence::archive::ArchiveError::ConflictingObservation
                        | crate::persistence::archive::ArchiveError::InvalidRecord
                )
            ));
            assert!(
                owner
                    .open()
                    .await
                    .unwrap()
                    .ready_sources()
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        owner.shutdown().await;
    });
}
