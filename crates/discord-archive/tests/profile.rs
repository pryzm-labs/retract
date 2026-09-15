mod common;
use common::*;
use discord_archive::{
    ArchiveError, ArchiveInventory, ArchiveLimits, DiscordProfile, ProfileInspection,
};
use std::io::Cursor;

// Observe actual allocations on this test thread only; never intercept or fail
// allocations. A missing pre-allocation budget check must be externally visible.
struct AllocationObserver;
std::thread_local! {
    static WATCH_ALLOCATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LARGEST_ALLOCATION: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static REALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn observe_allocation(bytes: usize) {
    if WATCH_ALLOCATIONS
        .try_with(|watch| watch.get())
        .unwrap_or(false)
    {
        let _ = LARGEST_ALLOCATION.try_with(|largest| largest.set(largest.get().max(bytes)));
    }
}
unsafe impl std::alloc::GlobalAlloc for AllocationObserver {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        observe_allocation(layout.size());
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        observe_allocation(size);
        if WATCH_ALLOCATIONS
            .try_with(|watch| watch.get())
            .unwrap_or(false)
        {
            let _ = REALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
        unsafe { std::alloc::System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: AllocationObserver = AllocationObserver;

// Profile validation aggregates several entries, unlike a single-node probe.
fn limits() -> ArchiveLimits {
    ArchiveLimits {
        max_structure_bytes: 100_000,
        ..common::limits()
    }
}

const ACCOUNT: &str = r#"{"id":"9007199254741001","username":"invented_owner"}"#;
const CHANNEL: &str = r#"{"id":"9007199254741101","type":"invented_kind","recipients":["9007199254741001","invented_unknown"]}"#;
const ROWS: &str = r#"[{"ID":9007199254741201,"Timestamp":"2030-01-02 03:04:05","Contents":"invented text","Attachments":""}]"#;

fn entries() -> Vec<(String, String)> {
    [
        ("Account/user.json", ACCOUNT),
        ("Messages/index.json", "{}"),
        ("Messages/c9007199254741101/channel.json", CHANNEL),
        ("Messages/c9007199254741101/messages.json", ROWS),
    ]
    .map(|(name, body)| (name.into(), body.into()))
    .to_vec()
}
fn detect(
    entries: &[(String, String)],
    policy: ArchiveLimits,
) -> Result<ProfileInspection, ArchiveError> {
    let borrowed: Vec<_> = entries
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_bytes()))
        .collect();
    let mut archive = ArchiveInventory::inspect(Cursor::new(zip(&borrowed)), policy, &NeverCancel)?;
    DiscordProfile::detect(&mut archive)
}
fn rejected(entries: &[(String, String)]) {
    let result = detect(entries, limits());
    assert!(
        matches!(
            result,
            Err(ArchiveError::InvalidProfile | ArchiveError::UnsupportedProfile)
        ),
        "{result:?}"
    );
}

#[test]
fn exact_profile_returns_lossless_typed_headers_and_entry_indexes_only() {
    let profile = detect(&entries(), limits()).unwrap();
    assert_eq!(profile.schema_key, "discord.data_package.messages_json");
    assert_eq!(profile.schema_version, 1);
    assert_eq!(profile.policy_key, "discord.import_policy.v1");
    assert_eq!(profile.account.id, "9007199254741001");
    assert_eq!(profile.account.username, "invented_owner");
    assert_eq!(profile.contexts[0].header.id, "9007199254741101");
    assert_eq!(profile.contexts[0].messages_entry.0, 3);
    let debug = format!("{profile:?}");
    for private in [
        "9007199254741001",
        "9007199254741101",
        "invented_owner",
        "invented_kind",
        "invented text",
    ] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn missing_duplicate_and_contradictory_identity_fail_closed() {
    let mut missing = entries();
    missing.remove(0);
    rejected(&missing);
    for account in [
        r#"{"username":"invented_owner"}"#,
        r#"{"id":9007199254741001,"username":"invented_owner"}"#,
        r#"{"id":"9007199254741001","id":"9007199254741002","username":"invented_owner"}"#,
        r#"{"id":"0","username":"invented_owner"}"#,
    ] {
        let mut e = entries();
        e[0].1 = account.into();
        rejected(&e);
    }
    let mut e = entries();
    e.push(("Account/account.json".into(), ACCOUNT.into()));
    rejected(&e);
    let mut e = entries();
    e[2].1 = CHANNEL.replace("9007199254741101", "9007199254741102");
    rejected(&e);
}

#[test]
fn exact_roots_required_files_csv_and_wrappers_fail_closed() {
    for (index, replacement) in [
        (0, "account/user.json"),
        (0, "wrapper/Account/user.json"),
        (2, "Messages/9007199254741101/channel.json"),
        (3, "Messages/c9007199254741101/messages.csv"),
        (3, "Messages/c09007199254741101/messages.json"),
    ] {
        let mut e = entries();
        e[index].0 = replacement.into();
        rejected(&e);
    }
    for index in 1..4 {
        let mut e = entries();
        e.remove(index);
        rejected(&e);
    }
    for body in ["{}", r#"{"messages":[]}"#, "[{},[]]", "[null]"] {
        let mut e = entries();
        e[3].1 = body.into();
        rejected(&e);
    }
    let mut e = entries();
    e.push(("wrapper/Account/user.json".into(), ACCOUNT.into()));
    rejected(&e);
}

#[test]
fn all_required_row_fields_types_and_original_id_tokens_are_exact() {
    for field in ["ID", "Timestamp", "Contents", "Attachments"] {
        let mut row: serde_json::Value = serde_json::from_str(ROWS).unwrap();
        row[0].as_object_mut().unwrap().remove(field);
        let mut e = entries();
        e[3].1 = row.to_string();
        rejected(&e);
        let mut row: serde_json::Value = serde_json::from_str(ROWS).unwrap();
        row[0][field] = serde_json::Value::Null;
        e[3].1 = row.to_string();
        rejected(&e);
    }
    for id in [
        "0",
        "-0",
        "-1",
        "1.0",
        "1e2",
        "18446744073709551616",
        "\"9007199254741201\"",
    ] {
        let mut e = entries();
        e[3].1 = ROWS.replace("9007199254741201", id);
        rejected(&e);
    }
    for id in ["1", "9007199254740993", "18446744073709551615"] {
        let mut e = entries();
        e[3].1 = ROWS.replace("9007199254741201", id);
        detect(&e, limits()).unwrap();
    }
    let mut e = entries();
    e[3].1 = ROWS.replace("\"Contents\":", "\"contents\":");
    rejected(&e);
    let mut e = entries();
    e[3].1 = format!("[{},{{\"ID\":9007199254741202}}]", &ROWS[1..ROWS.len() - 1]);
    rejected(&e);
}

#[test]
fn timestamps_are_calendar_validated_seconds_and_explicitly_unzoned() {
    for timestamp in [
        "2030-01-02T03:04:05",
        "2030-01-02 03:04:05Z",
        "2030-01-02 03:04:05+00:00",
        "2030-01-02 03:04:05.000",
        "2030-02-29 03:04:05",
        "2030-01-02 03:04:60",
        " 2030-01-02 03:04:05",
    ] {
        let mut e = entries();
        e[3].1 = ROWS.replace("2030-01-02 03:04:05", timestamp);
        rejected(&e);
    }
    let mut e = entries();
    e[3].1 = ROWS.replace("2030-01-02 03:04:05", "2040-02-29 23:59:59");
    detect(&e, limits()).unwrap();
}

#[test]
fn structural_context_variants_and_empty_transcripts_are_supported() {
    for tail in [
        "",
        r#", "name":null,"recipients":[]"#,
        r#", "name":"invented_group","recipients":["invented_recipient"]"#,
        r#", "name":"invented_channel","guild":{"id":"9007199254741301","name":"invented_guild"}"#,
    ] {
        let mut e = entries();
        e[2].1 = format!(r#"{{"id":"9007199254741101","type":"opaque_invented"{tail}}}"#);
        e[3].1 = "[]".into();
        detect(&e, limits()).unwrap();
    }
    for tail in [
        r#", "recipients":null"#,
        r#", "guild":null"#,
        r#", "guild":{"id":"1","name":"invented"},"recipients":[]"#,
        r#", "recipients":[1]"#,
        r#", "name":42"#,
    ] {
        let mut e = entries();
        e[2].1 = format!(r#"{{"id":"9007199254741101","type":"opaque_invented"{tail}}}"#);
        rejected(&e);
    }
}

#[test]
fn bounded_unknown_fields_and_sections_are_ignored_without_weakening_limits() {
    let mut e = entries();
    e[0].1 = ACCOUNT.replace('}', ",\"invented_extra\":[true,{\"nested\":null}]}");
    e.push(("invented-section/notes.txt".into(), "invented notes".into()));
    detect(&e, limits()).unwrap();
    let mut policy = limits();
    policy.max_scalar_bytes = 8;
    assert_eq!(detect(&e, policy).unwrap_err(), ArchiveError::LimitExceeded);
    let mut policy = limits();
    policy.max_json_tokens = 5;
    assert_eq!(detect(&e, policy).unwrap_err(), ArchiveError::LimitExceeded);
    let mut policy = limits();
    policy.max_selected_contexts = 1;
    assert_eq!(detect(&e, policy).unwrap_err(), ArchiveError::LimitExceeded);
}

#[test]
fn committed_synthetic_fixture_freezes_all_headers_without_decoding_messages() {
    let bytes = include_bytes!("../../../src-tauri/test-fixtures/discord-import/current-json.zip");
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(bytes), ArchiveLimits::default(), &NeverCancel)
            .unwrap();
    let profile = DiscordProfile::detect(&mut archive).unwrap();
    assert_eq!(profile.contexts.len(), 5);
    assert_eq!(profile.contexts[0].header.name, None);
    assert!(profile.contexts[0].header.recipients.is_some());
    assert_eq!(
        profile.contexts[1].header.name.as_deref(),
        Some("Invented Group")
    );
    assert_eq!(
        profile.contexts[2].header.guild.as_ref().unwrap().id,
        "9007199254741301"
    );
    assert!(profile.contexts[3].header.recipients.is_none());
    assert!(profile.contexts[4].header.name.is_none());
    for context in &profile.contexts {
        assert!(context.header.id.parse::<u64>().unwrap() > 9_007_199_254_740_991);
        assert!(!format!("{:?}", context.header).contains("Invented"));
    }
}

#[test]
fn cancellation_crc_and_all_resource_ceilings_survive_profile_validation() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Flag(AtomicBool);
    impl discord_archive::Cancellation for Flag {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let e = entries();
    let borrowed: Vec<_> = e
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_bytes()))
        .collect();
    let bytes = zip(&borrowed);
    let flag = Flag(AtomicBool::new(false));
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(bytes.clone()), limits(), &flag).unwrap();
    flag.0.store(true, Ordering::Relaxed);
    assert_eq!(
        DiscordProfile::detect(&mut archive).unwrap_err(),
        ArchiveError::Cancelled
    );
    let mut corrupted = bytes;
    let at = corrupted
        .windows(b"invented_owner".len())
        .position(|part| part == b"invented_owner")
        .unwrap();
    corrupted[at] = b'x';
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(corrupted), limits(), &NeverCancel).unwrap();
    assert_eq!(
        DiscordProfile::detect(&mut archive).unwrap_err(),
        ArchiveError::IntegrityFailure
    );
    for field in 0..5 {
        let mut policy = limits();
        match field {
            0 => policy.max_raw_record_bytes = 20,
            1 => policy.max_decoded_record_bytes = 20,
            2 => policy.max_json_depth = 1,
            3 => policy.max_structure_bytes = 200,
            _ => policy.max_observed_bytes = 100,
        }
        assert_eq!(detect(&e, policy).unwrap_err(), ArchiveError::LimitExceeded);
    }
}

#[test]
fn token_budget_is_shared_across_structure_and_typed_header_passes() {
    let e = entries();
    let mut policy = limits();
    policy.max_json_tokens = 34;
    assert_eq!(detect(&e, policy).unwrap_err(), ArchiveError::LimitExceeded);
}

#[test]
fn unknown_header_arrays_are_skipped_without_materializing_a_value_tree() {
    let mut e = entries();
    e[0].1 = ACCOUNT.replace(
        '}',
        &format!(",\"invented_extra\":[{}]}}", vec!["null"; 1000].join(",")),
    );
    let mut policy = limits();
    policy.max_json_tokens = 10_000;
    policy.max_structure_bytes = 50_000;
    let profile = detect(&e, policy).unwrap();
    assert_eq!(profile.account.username, "invented_owner");
}

#[test]
fn index_root_lookalikes_cannot_create_a_second_archive_root() {
    let mut e = entries();
    e.push(("wrapper/Messages/index.json".into(), "{}".into()));
    rejected(&e);
    for name in ["messages/index.json", "Messages/Index.json"] {
        let mut e = entries();
        e[1].0 = name.into();
        if name.starts_with("messages/") {
            for (path, _) in &mut e {
                *path = path.replacen("Messages/", "messages/", 1);
            }
        }
        rejected(&e);
        // A simultaneous case collision is already rejected by ZIP inventory.
        e.push(("Messages/index.json".into(), "{}".into()));
        assert!(detect(&e, limits()).is_err());
    }
}

#[test]
fn retained_display_fields_enforce_exact_utf8_byte_boundaries() {
    for field in ["username", "name", "guild_name", "recipient", "type"] {
        for (at_limit, over_limit) in [("aaaa", "aaaaa"), ("éé", "ééa")] {
            for (text, accepted) in [(at_limit, true), (over_limit, false)] {
                let mut e = entries();
                let mut account = serde_json::json!({"id":"9007199254741001","username":"x"});
                let mut channel = serde_json::json!({"id":"9007199254741101","type":"x","name":"x","recipients":["x"]});
                match field {
                    "username" => account["username"] = text.into(),
                    "name" => channel["name"] = text.into(),
                    "type" => channel["type"] = text.into(),
                    "recipient" => channel["recipients"][0] = text.into(),
                    _ => {
                        channel.as_object_mut().unwrap().remove("recipients");
                        channel["guild"] = serde_json::json!({"id":"9007199254741301","name":text});
                    }
                }
                e[0].1 = account.to_string();
                e[2].1 = channel.to_string();
                let mut policy = limits();
                policy.max_display_bytes = 4;
                let result = detect(&e, policy);
                if accepted {
                    result.unwrap();
                } else {
                    assert_eq!(result.unwrap_err(), ArchiveError::LimitExceeded);
                }
            }
        }
    }
}

#[test]
fn committed_fixture_cannot_bypass_a_one_byte_display_limit() {
    let bytes = include_bytes!("../../../src-tauri/test-fixtures/discord-import/current-json.zip");
    let policy = ArchiveLimits {
        max_display_bytes: 1,
        ..ArchiveLimits::default()
    };
    let mut archive = ArchiveInventory::inspect(Cursor::new(bytes), policy, &NeverCancel).unwrap();
    assert_eq!(
        DiscordProfile::detect(&mut archive).unwrap_err(),
        ArchiveError::LimitExceeded
    );
}

#[test]
fn selective_headers_preserve_field_order_optional_nulls_and_duplicate_rejection() {
    let mut e = entries();
    e[0].1 = r#"{"unknown":[null,true,42,{"nested":["invented",[]]}],"username":"invented_owner","id":"9007199254741001"}"#.into();
    e[2].1 = r#"{"recipients":["invented"],"name":null,"type":"invented_kind","id":"9007199254741101","extra":{"mixed":[1,false,null]}}"#.into();
    let profile = detect(&e, limits()).unwrap();
    assert_eq!(profile.account.id, "9007199254741001");
    assert!(profile.contexts[0].header.name.is_none());
    assert_eq!(
        profile.contexts[0].header.recipients.as_ref().unwrap(),
        &["invented"]
    );
    e[2].1 = r#"{"guild":{"name":"invented_guild","unknown":[],"id":"9007199254741301"},"name":"invented_channel","id":"9007199254741101","type":"invented_kind"}"#.into();
    assert_eq!(
        detect(&e, limits()).unwrap().contexts[0]
            .header
            .guild
            .as_ref()
            .unwrap()
            .name,
        "invented_guild"
    );
    for (index, body) in [
        (
            0,
            r#"{"id":"9007199254741001","username":"x","username":"y"}"#,
        ),
        (
            2,
            r#"{"id":"9007199254741101","type":"x","name":null,"name":"y"}"#,
        ),
        (
            2,
            r#"{"id":"9007199254741101","type":"x","name":"y","guild":{"id":"9007199254741301","name":"a","name":"b"}}"#,
        ),
        (
            2,
            r#"{"id":"9007199254741101","type":"x","recipients":null}"#,
        ),
    ] {
        let mut e = entries();
        e[index].1 = body.into();
        rejected(&e);
    }
}

#[test]
fn selective_unknown_fields_still_enforce_scalar_depth_and_token_limits() {
    let mut e = entries();
    e[0].1 = ACCOUNT.replace(
        '}',
        &format!(
            ",\"unknown\":{}}}",
            serde_json::to_string(&"x".repeat(1001)).unwrap()
        ),
    );
    assert_eq!(
        detect(&e, limits()).unwrap_err(),
        ArchiveError::LimitExceeded
    );
    e[0].1 = ACCOUNT.replace(
        '}',
        &format!(",\"unknown\":{}null{}}}", "[".repeat(13), "]".repeat(13)),
    );
    assert_eq!(
        detect(&e, limits()).unwrap_err(),
        ArchiveError::LimitExceeded
    );
    e[0].1 = ACCOUNT.replace(
        '}',
        &format!(",\"unknown\":[{}]}}", vec!["null"; 1001].join(",")),
    );
    assert_eq!(
        detect(&e, limits()).unwrap_err(),
        ArchiveError::LimitExceeded
    );
}

#[test]
fn raw_header_growth_is_charged_before_allocating_beyond_the_budget() {
    let mut e = entries();
    e[0].1.insert_str(1, &" ".repeat(15_000));
    let borrowed: Vec<_> = e
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_bytes()))
        .collect();
    let policy = ArchiveLimits {
        max_structure_bytes: 10_000,
        max_raw_record_bytes: 20_000,
        max_observed_bytes: 100_000,
        ..limits()
    };
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(zip(&borrowed)), policy, &NeverCancel).unwrap();
    LARGEST_ALLOCATION.set(0);
    WATCH_ALLOCATIONS.set(true);
    let result = DiscordProfile::detect(&mut archive);
    WATCH_ALLOCATIONS.set(false);
    assert_eq!(result.unwrap_err(), ArchiveError::LimitExceeded);
    // The existing fixed 8 KiB lexical workspace fits this injected ceiling;
    // an uncharged read_to_end buffer does not.
    assert!(
        LARGEST_ALLOCATION.get() <= 10_000,
        "allocation exceeded injected budget"
    );
}

fn later_header_progress(
    username_bytes: usize,
    budget: u64,
) -> (Result<ProfileInspection, ArchiveError>, usize, usize) {
    use std::{
        cell::Cell,
        io::{Read, Seek, SeekFrom},
        rc::Rc,
    };
    struct ReadCounter {
        input: Cursor<Vec<u8>>,
        armed: Rc<Cell<bool>>,
        count: Rc<Cell<usize>>,
        start: u64,
        end: u64,
        pass: usize,
    }
    impl Read for ReadCounter {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let position = self.input.position();
            if self.armed.get() && position == self.start {
                self.pass += 1;
            }
            let read = self.input.read(out)?;
            if self.armed.get() && self.pass == 2 && (self.start..self.end).contains(&position) {
                self.count
                    .set(self.count.get() + read.min((self.end - position) as usize));
            }
            Ok(read)
        }
    }
    impl Seek for ReadCounter {
        fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
            self.input.seek(from)
        }
    }
    let mut e = entries();
    e[0].1 = ACCOUNT.replace("invented_owner", &"x".repeat(username_bytes));
    e[2].1.insert_str(1, &" ".repeat(6_000));
    let start = e[..2]
        .iter()
        .map(|(name, body)| 30 + name.len() + body.len())
        .sum::<usize>()
        + 30
        + e[2].0.len();
    let body_bytes = e[2].1.len();
    let borrowed: Vec<_> = e
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_bytes()))
        .collect();
    let armed = Rc::new(Cell::new(false));
    let count = Rc::new(Cell::new(0));
    let reader = ReadCounter {
        input: Cursor::new(zip(&borrowed)),
        armed: armed.clone(),
        count: count.clone(),
        start: start as u64,
        end: (start + body_bytes) as u64,
        pass: 0,
    };
    let policy = ArchiveLimits {
        max_structure_bytes: budget,
        ..limits()
    };
    let mut archive = ArchiveInventory::inspect(reader, policy, &NeverCancel).unwrap();
    armed.set(true);
    let result = DiscordProfile::detect(&mut archive);
    (result, count.get(), body_bytes)
}

#[test]
fn later_header_growth_uses_only_the_remaining_cumulative_budget() {
    let mut observed_remaining_budget_failure = false;
    for budget in (10_000..=30_000).step_by(500) {
        let (small, small_read, size) = later_header_progress(1, budget);
        let (large, large_read, _) = later_header_progress(512, budget);
        if matches!(small, Err(ArchiveError::LimitExceeded))
            && matches!(large, Err(ArchiveError::LimitExceeded))
            && 0 < large_read
            && large_read < small_read
            && small_read < size
        {
            observed_remaining_budget_failure = true;
        }
    }
    assert!(
        observed_remaining_budget_failure,
        "later header must stop incrementally as prior headers consume the budget"
    );
}

#[test]
fn retained_recipient_capacity_grows_geometrically_within_the_budget() {
    let mut e = entries();
    e[2].1 = format!(
        r#"{{"id":"9007199254741101","type":"x","recipients":[{}]}}"#,
        vec!["\"x\""; 512].join(",")
    );
    let borrowed: Vec<_> = e
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_bytes()))
        .collect();
    let policy = ArchiveLimits {
        max_json_tokens: 10_000,
        ..limits()
    };
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(zip(&borrowed)), policy, &NeverCancel).unwrap();
    REALLOCATIONS.set(0);
    WATCH_ALLOCATIONS.set(true);
    let result = DiscordProfile::detect(&mut archive);
    WATCH_ALLOCATIONS.set(false);
    assert_eq!(
        result.unwrap().contexts[0]
            .header
            .recipients
            .as_ref()
            .unwrap()
            .len(),
        512
    );
    assert!(
        REALLOCATIONS.get() < 100,
        "retaining recipients must not reallocate once per item"
    );
}
