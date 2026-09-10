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

fn scalar_grammars(report: &StructureReport, path: &[&str]) -> serde_json::Value {
    let encoded = serde_json::to_value(report).unwrap();
    encoded["entries"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["path"] == serde_json::json!(path))
        .unwrap()["grammars"]
        .clone()
}

fn string_grammars(value: &str) -> serde_json::Value {
    let payload = serde_json::to_vec(value).unwrap();
    scalar_grammars(&probe(&payload, limits()).unwrap(), &[])
}

fn observed(value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"Observed": [value]})
}

#[test]
fn decimal_strings_classify_exact_syntax_and_u64_boundaries() {
    // Permitting signs, rounding, trimming, or overflowing an ID breaks this table.
    for (input, expected) in [
        ("1", "CanonicalPositiveU64Decimal"),
        ("9007199254740993", "CanonicalPositiveU64Decimal"),
        ("18446744073709551615", "CanonicalPositiveU64Decimal"),
        ("18446744073709551616", "OutOfU64Range"),
        ("9999999999999999999999999999999999999999", "OutOfU64Range"),
        ("0", "Zero"),
        ("00", "NoncanonicalDecimal"),
        ("01", "NoncanonicalDecimal"),
        ("-0", "NoncanonicalDecimal"),
        ("-1", "NoncanonicalDecimal"),
        ("+1", "NoncanonicalDecimal"),
        ("1.0", "NoncanonicalDecimal"),
        (".5", "NoncanonicalDecimal"),
        ("1.", "NoncanonicalDecimal"),
        ("1e0", "NoncanonicalDecimal"),
        ("1E+2", "NoncanonicalDecimal"),
        ("1e-2", "NoncanonicalDecimal"),
        (" 1", "NoncanonicalDecimal"),
        ("1 ", "NoncanonicalDecimal"),
        ("\t\n1\r ", "NoncanonicalDecimal"),
        ("-18446744073709551616", "NoncanonicalDecimal"),
        ("01e1", "NoncanonicalDecimal"),
        ("", "Other"),
        (" ", "Other"),
        ("+", "Other"),
        ("-", "Other"),
        (".", "Other"),
        ("e1", "Other"),
        ("1e", "Other"),
        ("1e+", "Other"),
        ("1 2", "Other"),
        ("1\u{a0}", "Other"),
        ("１２", "Other"),
        ("0x12", "Other"),
        ("NaN", "Other"),
        ("Infinity", "Other"),
        ("1invented", "Other"),
    ] {
        let grammar = string_grammars(input);
        assert_eq!(grammar["decimal_strings"], observed(expected.into()));
        assert_eq!(grammar["decimal_numbers"], "NoObservation");
    }
}

#[test]
fn numeric_grammar_uses_original_tokens_without_rounding_or_normalizing() {
    for (input, expected) in [
        ("1", "CanonicalPositiveU64Decimal"),
        ("9007199254740993", "CanonicalPositiveU64Decimal"),
        ("18446744073709551615", "CanonicalPositiveU64Decimal"),
        ("18446744073709551616", "OutOfU64Range"),
        ("9999999999999999999999999999999999999999", "OutOfU64Range"),
        ("0", "Zero"),
        ("-0", "NoncanonicalDecimal"),
        ("-1", "NoncanonicalDecimal"),
        ("0.0", "NoncanonicalDecimal"),
        ("1.0", "NoncanonicalDecimal"),
        ("1e0", "NoncanonicalDecimal"),
        ("1E+2", "NoncanonicalDecimal"),
        ("1e-2", "NoncanonicalDecimal"),
        ("-18446744073709551616", "NoncanonicalDecimal"),
        ("18446744073709551615.0", "NoncanonicalDecimal"),
    ] {
        // JSON formatting whitespace is outside the numeric token.
        let payload = format!(" \n{input}\t ");
        let report = probe(payload.as_bytes(), limits()).unwrap();
        let grammar = scalar_grammars(&report, &[]);
        assert_eq!(grammar["decimal_numbers"], observed(expected.into()));
        assert_eq!(grammar["decimal_strings"], "NoObservation");
        assert_eq!(grammar["timestamps"], "NoObservation");
    }
    for input in ["01", "+1", "00", "1.", ".5", "1e", "--1"] {
        assert_eq!(
            probe(input.as_bytes(), limits()).unwrap_err(),
            ArchiveError::InvalidJson
        );
    }
}

#[test]
fn numeric_grammar_does_not_bleed_across_keys_containers_or_tokens() {
    let report = probe(
        br#"{"a":18446744073709551616,"1e0":[null,true,"-0",{"b":0}],"c":1e0,"d":9007199254740993}"#,
        limits(),
    ).unwrap();
    for (path, expected) in [
        (vec!["a"], "OutOfU64Range"),
        (vec!["1e0", "[]", "b"], "Zero"),
        (vec!["c"], "NoncanonicalDecimal"),
        (vec!["d"], "CanonicalPositiveU64Decimal"),
    ] {
        assert_eq!(
            scalar_grammars(&report, &path)["decimal_numbers"],
            observed(expected.into())
        );
    }
}

#[test]
fn timestamps_distinguish_every_closed_separator_precision_and_zone_form() {
    let mut inputs = Vec::new();
    let mut expected_forms = Vec::new();
    for (separator, separator_name) in [("T", "UpperT"), (" ", "Space")] {
        for (fraction, precision) in [
            ("", "Seconds"),
            (".123", "Milliseconds"),
            (".123456", "Microseconds"),
            (".123456789", "Nanoseconds"),
        ] {
            for (suffix, zone) in [("Z", "UpperZ"), ("+02:30", "ColonOffset"), ("", "Unzoned")] {
                let input = format!("2040-02-29{separator}12:34:56{fraction}{suffix}");
                let expected = serde_json::json!({"Calendar": {
                    "separator": separator_name, "precision": precision, "zone": zone
                }});
                assert_eq!(
                    string_grammars(&input)["timestamps"],
                    observed(expected.clone())
                );
                inputs.push(input);
                expected_forms.push(expected);
            }
        }
    }
    inputs.extend(inputs.clone().into_iter().rev());
    inputs.push("invented".into());
    expected_forms.push("Unclassified".into());
    let report = probe(&serde_json::to_vec(&inputs).unwrap(), limits()).unwrap();
    let grammar = scalar_grammars(&report, &["[]"]);
    let forms = grammar["timestamps"]["Observed"].as_array().unwrap();
    assert_eq!(forms.len(), 25);
    for expected in expected_forms {
        assert!(forms.contains(&expected));
    }
}

#[test]
fn timestamp_calendar_boundaries_and_offset_grammar_are_validated() {
    let valid = observed(serde_json::json!({"Calendar": {
        "separator": "UpperT", "precision": "Seconds", "zone": "UpperZ"
    }}));
    for input in [
        "0001-01-01T00:00:00Z",
        "9999-12-31T23:59:59Z",
        "2000-02-29T00:00:00Z",
        "2036-02-29T00:00:00Z",
        "2400-02-29T00:00:00Z",
        "1900-02-28T00:00:00Z",
    ] {
        assert_eq!(string_grammars(input)["timestamps"], valid);
    }
    // Literal month lengths make this independent of the classifier's calendar calculation.
    for (month, last_day) in [
        (1, 31),
        (2, 28),
        (3, 31),
        (4, 30),
        (5, 31),
        (6, 30),
        (7, 31),
        (8, 31),
        (9, 30),
        (10, 31),
        (11, 30),
        (12, 31),
    ] {
        let good = format!("2041-{month:02}-{last_day:02}T00:00:00Z");
        let bad = format!("2041-{month:02}-{:02}T00:00:00Z", last_day + 1);
        assert_eq!(string_grammars(&good)["timestamps"], valid);
        assert_eq!(
            string_grammars(&bad)["timestamps"],
            observed("Unclassified".into())
        );
    }
    for suffix in ["+00:00", "+23:59", "-23:59", "-00:01", "-01:00"] {
        let input = format!("2040-02-29T23:59:59{suffix}");
        assert_eq!(
            string_grammars(&input)["timestamps"],
            observed(serde_json::json!({"Calendar": {
                "separator": "UpperT", "precision": "Seconds", "zone": "ColonOffset"
            }}))
        );
    }
}

#[test]
fn unknown_or_invalid_timestamp_forms_remain_unclassified() {
    for input in [
        "0000-01-01T00:00:00Z",
        "10000-01-01T00:00:00Z",
        "1900-02-29T00:00:00Z",
        "2100-02-29T00:00:00Z",
        "2041-02-29T00:00:00Z",
        "2040-00-01T00:00:00Z",
        "2040-13-01T00:00:00Z",
        "2040-01-00T00:00:00Z",
        "2040-01-32T00:00:00Z",
        "2040-01-01T24:00:00Z",
        "2040-01-01T00:60:00Z",
        "2040-01-01T00:00:60Z",
        "2040-01-01T00:00:00-00:00",
        "2040-01-01T00:00:00+24:00",
        "2040-01-01T00:00:00+00:60",
        "2040-01-01T00:00:00+0000",
        "2040-01-01T00:00:00+00",
        "2040-01-01T00:00:00+00:00:00",
        "2040-01-01T00:00:00z",
        "2040-01-01t00:00:00Z",
        "2040-01-01_00:00:00Z",
        "2040-01-01T00:00:00.Z",
        "2040-01-01T00:00:00.1Z",
        "2040-01-01T00:00:00.12Z",
        "2040-01-01T00:00:00.1234Z",
        "2040-01-01T00:00:00.12345Z",
        "2040-01-01T00:00:00.1234567Z",
        "2040-01-01T00:00:00.12345678Z",
        "2040-01-01T00:00:00.1234567890Z",
        "2040-01-01T00:00:00,123Z",
        "2040-01-01T00:00:00.abcZ",
        "2040-01-01T00:00:00.１２３Z",
        " 2040-01-01T00:00:00Z",
        "2040-01-01T00:00:00Z ",
        "2040-01-01T00:00:00Z\n",
        "2040-1-01T00:00:00Z",
        "2040-01-1T00:00:00Z",
        "2040/01/01T00:00:00Z",
        "2040-01-01T0:00:00Z",
        "2040-01-01T00:0:00Z",
        "2040-01-01T00:00:0Z",
        "2040-01-01",
        "",
        "invented",
        "2040-01-01T00:00:00Zinvented",
    ] {
        assert_eq!(
            string_grammars(input)["timestamps"],
            observed("Unclassified".into())
        );
    }
}

#[test]
fn grammar_evidence_keeps_mixed_and_unobserved_states_distinct() {
    for input in ["null", "true", "false", "[]", "{}"] {
        assert_eq!(
            scalar_grammars(&probe(input.as_bytes(), limits()).unwrap(), &[]),
            serde_json::json!({
                "decimal_strings": "NoObservation", "decimal_numbers": "NoObservation", "timestamps": "NoObservation"
            })
        );
    }
    let report = probe(
        br#"[[],["9007199254740993","0","01","18446744073709551616","invented",0,1e0],["other invented text","999999999999999999999999","-1","0","18446744073709551615",0,2e0],[]]"#,
        limits(),
    )
    .unwrap();
    let grammar = scalar_grammars(&report, &["[]", "[]"]);
    assert_eq!(
        grammar["decimal_strings"],
        serde_json::json!({"Observed":[
            "CanonicalPositiveU64Decimal", "Zero", "NoncanonicalDecimal", "OutOfU64Range", "Other"
        ]})
    );
    assert_eq!(
        grammar["decimal_numbers"],
        serde_json::json!({"Observed":["Zero", "NoncanonicalDecimal"]})
    );
    assert_eq!(grammar["timestamps"], observed("Unclassified".into()));
    let report = probe(
        br#"["2040-01-01T00:00:00Z","invented",null,"2040-01-01 00:00:00"]"#,
        limits(),
    )
    .unwrap();
    let timestamps = scalar_grammars(&report, &["[]"])["timestamps"]["Observed"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(timestamps.len(), 3);
    assert!(timestamps.contains(&serde_json::json!("Unclassified")));
    assert!(timestamps.contains(&serde_json::json!({"Calendar":{"separator":"Space","precision":"Seconds","zone":"Unzoned"}})));
    assert!(timestamps.contains(&serde_json::json!({"Calendar":{"separator":"UpperT","precision":"Seconds","zone":"UpperZ"}})));
}

#[test]
fn path_grammar_recognizes_only_the_closed_c_prefix_and_canonical_decimal() {
    for (segment, expected) in [
        ("9007199254740993", "decimal_identifier"),
        ("18446744073709551615", "decimal_identifier"),
        (
            "c9007199254740993",
            "lowercase_c_prefixed_canonical_positive_u64_decimal",
        ),
        (
            "c18446744073709551615",
            "lowercase_c_prefixed_canonical_positive_u64_decimal",
        ),
        ("0", "mixed_identifier"),
        ("01", "mixed_identifier"),
        ("18446744073709551616", "mixed_identifier"),
        ("c0", "mixed_identifier"),
        ("c01", "mixed_identifier"),
        ("c18446744073709551616", "mixed_identifier"),
        ("C9007199254740993", "mixed_identifier"),
        ("x9007199254740993", "mixed_identifier"),
        ("cc9007199254740993", "mixed_identifier"),
        ("c+9007199254740993", "mixed_identifier"),
        ("c-9007199254740993", "mixed_identifier"),
        ("c9007199254740993x", "mixed_identifier"),
        ("c9007199254740993.0", "mixed_identifier"),
        ("c9007199254740993e0", "mixed_identifier"),
        ("c", "redacted_segment"),
    ] {
        let path = format!("messages/{segment}/messages.json");
        let mut archive =
            ArchiveInventory::inspect(Cursor::new(zip(&[(&path, b"[]")])), limits(), &NeverCancel)
                .unwrap();
        let report = StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap();
        assert_eq!(
            serde_json::to_value(&report).unwrap()["entries"][0]["path"][1],
            expected
        );
    }
}

#[test]
fn equal_grammar_from_distinct_invented_values_is_identical_and_value_free() {
    let mut reports = Vec::new();
    for (path, payload) in [
        ("messages/c9007199254740993/messages.json", br#"{"decimal":"9007199254740993","numeric":9007199254740993,"time":"2040-02-29T01:02:03.123+01:00","text":"SENTINEL_ALPHA_730145","url":"https://example.invalid/invented-alpha"}"#.as_slice()),
        ("messages/c18446744073709551615/messages.json", br#"{"decimal":"18446744073709551615","numeric":18446744073709551615,"time":"2088-12-31T23:59:59.987-07:30","text":"SENTINEL_BETA_893501_WITH_DIFFERENT_SIZE","url":"https://example.invalid/invented-beta-longer"}"#.as_slice()),
    ] {
        let mut archive = ArchiveInventory::inspect(Cursor::new(zip(&[(path, payload)])), limits(), &NeverCancel).unwrap();
        let report = StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap();
        let json = serde_json::to_string(&report).unwrap();
        let debug = format!("{report:?}");
        for sentinel in ["9007199254740993", "18446744073709551615", "2040-02-29", "2088-12-31", "+01:00", "-07:30", "SENTINEL_", "example.invalid", "invented-alpha", "invented-beta"] {
            assert!(!json.contains(sentinel), "JSON leaked a synthetic sentinel");
            assert!(!debug.contains(sentinel), "Debug leaked a synthetic sentinel");
        }
        reports.push(json);
    }
    assert_eq!(reports[0], reports[1]);
    assert!(reports[0].contains("CanonicalPositiveU64Decimal"));
    assert!(reports[0].contains("Milliseconds"));
}

#[test]
fn exact_path_candidates_distinguish_case_and_filename_boundaries() {
    for (path, expected) in [
        ("Account/user.json", vec!["title_case_account", "user_json"]),
        ("account/user.json", vec!["account", "user_json"]),
        (
            "Messages/index.json",
            vec!["title_case_messages", "index_json"],
        ),
        ("messages/index.json", vec!["messages", "index_json"]),
        (
            "Messages/c9007199254740993/messages.json",
            vec![
                "title_case_messages",
                "lowercase_c_prefixed_canonical_positive_u64_decimal",
                "messages_json",
            ],
        ),
        ("user.json", vec!["user_json"]),
    ] {
        let mut archive =
            ArchiveInventory::inspect(Cursor::new(zip(&[(path, b"{}")])), limits(), &NeverCancel)
                .unwrap();
        let report = StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap()["entries"][0]["path"],
            serde_json::json!(expected)
        );
    }
}

#[test]
fn path_candidate_lookalikes_and_unknown_literals_stay_redacted() {
    for path in [
        "ACCOUNT/User.json",
        "aCcount/USER.JSON",
        "Accounts/users.json",
        "XAccount/user.jsonx",
        "AccountX/.user.json",
        " Account/user .json",
        "MESSAGES/unknown.json",
        "mEssages/private.json",
        "MessagesX/missing.json",
        "XMessages/other.json",
        "MessagesBackup/user.JSON",
        "ArbitraryRoot/UnknownFile.json",
        "Unknown/user-json",
        "UnlistedPrefix/UnknownFilename",
    ] {
        let mut archive =
            ArchiveInventory::inspect(Cursor::new(zip(&[(path, b"{}")])), limits(), &NeverCancel)
                .unwrap();
        let report = StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap()["entries"][0]["path"],
            serde_json::json!(["redacted_segment", "redacted_segment"])
        );
    }
    // Recognition is per segment; none of these paths is an accepted profile rule.
    for (path, expected) in [
        ("Unknown/user.json", ["redacted_segment", "user_json"]),
        (
            "Account/unknown.json",
            ["title_case_account", "redacted_segment"],
        ),
        (
            "Messages/unknown.json",
            ["title_case_messages", "redacted_segment"],
        ),
    ] {
        let mut archive =
            ArchiveInventory::inspect(Cursor::new(zip(&[(path, b"{}")])), limits(), &NeverCancel)
                .unwrap();
        let report = StructureProbe::inspect(&mut archive, &[EntryIndex(0)]).unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap()["entries"][0]["path"],
            serde_json::json!(expected)
        );
    }
}

#[test]
fn path_candidates_preserve_equal_shape_json_and_debug_non_disclosure() {
    let mut representations = Vec::new();
    for (unknown_path, payload) in [
        (
            "Messages/c9007199254740993/PRIVATE_ALPHA_FILE.json",
            br#"{"field":"SYNTHETIC_ALPHA_PRIVATE_VALUE"}"#.as_slice(),
        ),
        (
            "Messages/c18446744073709551615/PRIVATE_BETA_LONGER_FILE.json",
            br#"{"field":"SYNTHETIC_BETA_PRIVATE_VALUE_OF_DIFFERENT_SIZE"}"#.as_slice(),
        ),
    ] {
        let mut archive = ArchiveInventory::inspect(
            Cursor::new(zip(&[
                ("Account/user.json", payload),
                (unknown_path, payload),
            ])),
            limits(),
            &NeverCancel,
        )
        .unwrap();
        let report =
            StructureProbe::inspect(&mut archive, &[EntryIndex(0), EntryIndex(1)]).unwrap();
        let json = serde_json::to_string(&report).unwrap();
        let debug = format!("{report:?}");
        assert!(json.contains("title_case_account"));
        assert!(json.contains("title_case_messages"));
        assert!(json.contains("user_json"));
        for sentinel in [
            "9007199254740993",
            "18446744073709551615",
            "PRIVATE_ALPHA_FILE",
            "PRIVATE_BETA_LONGER_FILE",
            "SYNTHETIC_ALPHA_PRIVATE_VALUE",
            "SYNTHETIC_BETA_PRIVATE_VALUE_OF_DIFFERENT_SIZE",
            "Account/user.json",
            unknown_path,
        ] {
            assert!(!json.contains(sentinel), "JSON leaked a synthetic sentinel");
            assert!(
                !debug.contains(sentinel),
                "Debug leaked a synthetic sentinel"
            );
        }
        representations.push((json, debug));
    }
    assert_eq!(representations[0], representations[1]);
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
