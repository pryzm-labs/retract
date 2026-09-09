mod common;
use common::*;
use discord_archive::{
    ArchiveError, ArchiveInventory, ArchiveLimits, EntryIndex, JsonShape, StructureProbe,
    StructureReport,
};
use std::io::Cursor;

fn probe(payload: &[u8], policy: ArchiveLimits) -> Result<StructureReport, ArchiveError> {
    let mut archive = ArchiveInventory::inspect(
        Cursor::new(zip(&[(
            "messages/912345678901234567/SECRET_ACCOUNT_9284.json",
            payload,
        )])),
        policy,
        &NeverCancel,
    )?;
    StructureProbe::inspect(&mut archive, &[EntryIndex(0)])
}

#[test]
fn report_retains_shapes_and_keys_but_never_scalar_values_or_private_paths() {
    let report = probe(br#"[{"text":"SENTINEL_PRIVATE_BODY","id":987654321012345678,"url":"https://private.example/tokenSECRET","timestamp":"2042-01-02T03:04:05Z","flag":true,"nullable":null,"nested":[-928461,2.584739]}]"#, limits()).unwrap();
    let JsonShape::Array(item) = &report.entries[0].shape else {
        panic!("expected array")
    };
    let JsonShape::Object(fields) = item.as_ref() else {
        panic!("expected object")
    };
    assert_eq!(fields["text"], JsonShape::String);
    assert_eq!(
        fields["id"],
        JsonShape::Number {
            integer: true,
            signed: false
        }
    );
    assert_eq!(fields["flag"], JsonShape::Boolean);
    assert_eq!(fields["nullable"], JsonShape::Null);
    assert_eq!(report.entries[0].max_depth, 4);
    let encoded = serde_json::to_string(&report).unwrap();
    for sentinel in [
        "SENTINEL_PRIVATE_BODY",
        "987654321012345678",
        "private.example",
        "tokenSECRET",
        "2042-01-02",
        "928461",
        "2.584739",
        "912345678901234567",
        "SECRET_ACCOUNT_9284",
    ] {
        assert!(!encoded.contains(sentinel), "leaked sentinel");
    }
    assert!(encoded.contains("decimal_identifier"));
    assert!(encoded.contains("mixed_identifier"));
}

#[test]
fn heterogeneous_arrays_preserve_scalar_sets_and_optional_null_occurrences() {
    let report = probe(
        br#"[{"optional":null,"value":true},{"value":"secret"},{"optional":7,"value":false}]"#,
        limits(),
    )
    .unwrap();
    let nodes = &report.entries[0].nodes;
    let value = nodes
        .iter()
        .find(|node| node.path == ["[]", "value"])
        .unwrap();
    assert_eq!(value.occurrences, 3);
    assert_eq!(value.types.boolean, 2);
    assert_eq!(value.types.string, 1);
    let optional = nodes
        .iter()
        .find(|node| node.path == ["[]", "optional"])
        .unwrap();
    assert_eq!(optional.occurrences, 2);
    assert_eq!(optional.types.null, 1);
    assert_eq!(optional.missing, 1);
    let JsonShape::Array(item) = &report.entries[0].shape else {
        panic!()
    };
    let JsonShape::Object(fields) = item.as_ref() else {
        panic!()
    };
    assert_eq!(fields["value"], JsonShape::Mixed);
}

#[test]
fn empty_containers_and_scalar_roots_are_valid_structures() {
    for (input, expected) in [
        (b"null".as_slice(), JsonShape::Null),
        (b"[]", JsonShape::Array(Box::new(JsonShape::Mixed))),
        (b"{}", JsonShape::Object(Default::default())),
    ] {
        assert_eq!(probe(input, limits()).unwrap().entries[0].shape, expected);
    }
}

fn observed_object_item_shape() -> JsonShape {
    JsonShape::Object(std::collections::BTreeMap::from([(
        "field".to_owned(),
        JsonShape::Number {
            integer: true,
            signed: false,
        },
    )]))
}

#[test]
fn empty_array_before_observed_items_preserves_their_shape() {
    let report = probe(br#"[[],[{"field":1}]]"#, limits()).unwrap();
    assert_eq!(
        report.entries[0].shape,
        JsonShape::Array(Box::new(JsonShape::Array(Box::new(
            observed_object_item_shape()
        ))))
    );
}

#[test]
fn empty_array_after_observed_items_preserves_their_shape() {
    let report = probe(br#"[[{"field":1}],[]]"#, limits()).unwrap();
    assert_eq!(
        report.entries[0].shape,
        JsonShape::Array(Box::new(JsonShape::Array(Box::new(
            observed_object_item_shape()
        ))))
    );
}

#[test]
fn empty_array_fields_in_merged_objects_preserve_observed_items() {
    for input in [
        br#"[{"items":[]},{"items":[{"field":1}]}]"#.as_slice(),
        br#"[{"items":[{"field":1}]},{"items":[]}]"#,
    ] {
        let report = probe(input, limits()).unwrap();
        let JsonShape::Array(item) = &report.entries[0].shape else {
            panic!("expected array")
        };
        let JsonShape::Object(fields) = item.as_ref() else {
            panic!("expected object")
        };
        assert_eq!(
            fields["items"],
            JsonShape::Array(Box::new(observed_object_item_shape()))
        );
    }
}

#[test]
fn empty_array_merging_keeps_genuinely_mixed_items_mixed() {
    for input in [
        b"[[],[1,true],[1]]".as_slice(),
        b"[[1],[1,true],[]]",
        b"[[1,true],[],[1]]",
    ] {
        assert_eq!(
            probe(input, limits()).unwrap().entries[0].shape,
            JsonShape::Array(Box::new(JsonShape::Array(Box::new(JsonShape::Mixed))))
        );
    }
    assert_eq!(
        probe(b"[[],[]]", limits()).unwrap().entries[0].shape,
        JsonShape::Array(Box::new(JsonShape::Array(Box::new(JsonShape::Mixed))))
    );
}

#[test]
fn duplicate_keys_invalid_utf8_and_invalid_json_fail_without_snippets() {
    for input in [
        br#"{"key":1,"key":2}"#.as_slice(),
        br#"{"nested":{"key":1,"key":2}}"#,
    ] {
        assert_eq!(
            probe(input, limits()).unwrap_err(),
            ArchiveError::DuplicateJsonKey
        );
    }
    for input in [
        b"\xff".as_slice(),
        b"{\"text\":\"\xff\"}",
        b"[1] extraSECRET",
        br#"{"x":"unterminatedSECRET}"#,
    ] {
        assert_eq!(
            probe(input, limits()).unwrap_err(),
            ArchiveError::InvalidJson
        );
    }
}

#[test]
fn scalar_limits_cover_strings_numbers_keys_and_escaped_decoded_values() {
    let mut policy = limits();
    policy.max_scalar_bytes = 3;
    for input in [
        br#""four""#.as_slice(),
        br#"{"four":0}"#,
        b"1234",
        br#""\u0061\u0062\u0063\u0064""#,
    ] {
        assert_eq!(
            probe(input, policy).unwrap_err(),
            ArchiveError::LimitExceeded
        );
    }
}

#[test]
fn raw_decoded_token_and_depth_limits_apply_to_ignored_content() {
    let mut policy = limits();
    policy.max_raw_record_bytes = 9;
    assert_eq!(
        probe(br#"[{"x":"12345"}]"#, policy).unwrap_err(),
        ArchiveError::LimitExceeded
    );
    let mut policy = limits();
    policy.max_decoded_record_bytes = 5;
    assert_eq!(
        probe(br#"[{"x":"12345"}]"#, policy).unwrap_err(),
        ArchiveError::LimitExceeded
    );
    let mut policy = limits();
    policy.max_json_tokens = 4;
    assert_eq!(
        probe(b"[1,2,3,4,5]", policy).unwrap_err(),
        ArchiveError::LimitExceeded
    );
    let mut policy = limits();
    policy.max_json_depth = 3;
    assert_eq!(
        probe(br#"{"ignored":{"x":{"y":{"z":0}}}}"#, policy).unwrap_err(),
        ArchiveError::LimitExceeded
    );
}

#[test]
fn decoded_record_budget_accounts_for_numeric_storage() {
    let mut policy = limits();
    policy.max_decoded_record_bytes = 8;
    assert_eq!(
        probe(br#"{"n":12345}"#, policy).unwrap_err(),
        ArchiveError::LimitExceeded
    );
    assert!(probe(b"[12345,12345]", policy).is_ok());
}

#[test]
fn record_budgets_reset_between_streamed_items() {
    let mut policy = limits();
    policy.max_raw_record_bytes = 16;
    policy.max_decoded_record_bytes = 8;
    let report = probe(br#"[{"x":"12345"},{"x":"12345"},{"x":"12345"}]"#, policy).unwrap();
    assert_eq!(
        report.entries[0]
            .nodes
            .iter()
            .find(|node| node.path == ["[]"])
            .unwrap()
            .occurrences,
        3
    );
}

#[test]
fn retained_structure_budget_bounds_unique_keys_across_records() {
    let mut policy = limits();
    policy.max_structure_bytes = 60;
    assert_eq!(
        probe(br#"[{"first":0},{"second":0},{"third":0}]"#, policy).unwrap_err(),
        ArchiveError::LimitExceeded
    );
}

#[test]
fn selection_limits_directories_duplicates_and_bad_indexes_are_rejected() {
    let mut policy = limits();
    policy.max_selected_contexts = 1;
    let mut archive = ArchiveInventory::inspect(
        Cursor::new(zip(&[("a.json", b"{}"), ("b.json", b"{}"), ("data/", b"")])),
        policy,
        &NeverCancel,
    )
    .unwrap();
    assert_eq!(
        StructureProbe::inspect(&mut archive, &[EntryIndex(0), EntryIndex(1)]).unwrap_err(),
        ArchiveError::LimitExceeded
    );
    assert_eq!(
        StructureProbe::inspect(&mut archive, &[EntryIndex(9)]).unwrap_err(),
        ArchiveError::InvalidSelection
    );
    assert_eq!(
        StructureProbe::inspect(&mut archive, &[EntryIndex(2)]).unwrap_err(),
        ArchiveError::InvalidSelection
    );
    let mut archive = ArchiveInventory::inspect(
        Cursor::new(zip(&[("a.json", b"{}")])),
        limits(),
        &NeverCancel,
    )
    .unwrap();
    assert_eq!(
        StructureProbe::inspect(&mut archive, &[EntryIndex(0), EntryIndex(0)]).unwrap_err(),
        ArchiveError::InvalidSelection
    );
}

#[test]
fn cancellation_during_json_read_and_late_crc_prevent_a_report() {
    use discord_archive::Cancellation;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CancelAfter(AtomicUsize);
    impl Cancellation for CancelAfter {
        fn is_cancelled(&self) -> bool {
            self.0.fetch_add(1, Ordering::Relaxed) > 80
        }
    }
    let flag = CancelAfter(AtomicUsize::new(0));
    let payload = format!("[{}]", vec!["0"; 300].join(","));
    let mut archive = ArchiveInventory::inspect(
        Cursor::new(zip(&[("data.json", payload.as_bytes())])),
        limits(),
        &flag,
    )
    .unwrap();
    assert_eq!(
        StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap_err(),
        ArchiveError::Cancelled
    );
    assert!(!archive.is_validated(EntryIndex(0)));
    let mut bytes = zip(&[("x", b"{} ")]);
    bytes[33] = b'\n';
    let mut archive =
        ArchiveInventory::inspect(Cursor::new(bytes), limits(), &NeverCancel).unwrap();
    assert_eq!(
        StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap_err(),
        ArchiveError::IntegrityFailure
    );
    assert!(!archive.is_validated(EntryIndex(0)));
}

#[test]
fn unknown_path_segments_are_always_templated() {
    let mut archive = ArchiveInventory::inspect(
        Cursor::new(zip(&[(
            "ALICE_PRIVATE/messages/c9123456789/SECRET_NAME.json",
            b"{}",
        )])),
        limits(),
        &NeverCancel,
    )
    .unwrap();
    let report = StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap();
    let encoded = serde_json::to_string(&report).unwrap();
    for sentinel in ["ALICE_PRIVATE", "c9123456789", "SECRET_NAME"] {
        assert!(!encoded.contains(sentinel));
    }
    assert!(encoded.contains("messages"));
}

#[test]
fn private_probe_cli_accepts_one_regular_file_and_refuses_other_inputs() {
    use std::{fs, process::Command};
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/structure_probe");
    let root = std::env::temp_dir().join(format!("discord-probe-synthetic-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let input = root.join("synthetic.zip");
    fs::write(
        &input,
        zip(&[
            (
                "messages/ALICE_PRIVATE/data.json",
                br#"{"text":"SECRET_BODY"}"#,
            ),
            ("unknown.bin", b"not json"),
        ]),
    )
    .unwrap();
    let result = Command::new(&executable).arg(&input).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(!stdout.contains("SECRET_BODY"));
    assert!(!stdout.contains("ALICE_PRIVATE"));
    assert!(result.stderr.is_empty());
    for args in [
        vec![],
        vec!["-".into()],
        vec![root.clone().into_os_string()],
        vec![
            input.clone().into_os_string(),
            root.join("output.json").into_os_string(),
        ],
    ] {
        let result = Command::new(&executable).args(args).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&result.stderr).contains(root.to_str().unwrap()));
    }
    #[cfg(unix)]
    {
        let link = root.join("link.zip");
        std::os::unix::fs::symlink(&input, &link).unwrap();
        let result = Command::new(&executable).arg(&link).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
    assert!(!root.join("output.json").exists());
    fs::remove_dir_all(root).unwrap();
}
