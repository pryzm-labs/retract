use chrono::{DateTime, Utc};
use retract_domain::*;
use serde_json::json;
use uuid::Uuid;

use crate::persistence::ProviderPayloadValidator;
use crate::providers::discord::model::{DiscordSourceProfile, VerifiedConversationKind};
use crate::providers::discord::{
    DiscordNormalizer, DiscordPayloadValidator,
    locators::{
        DiscordChannelLocator, DiscordMessageLocator, DiscordUserLocator, discord_provider_key,
    },
};
use discord_archive::{ChannelContext, DiscordId, ExportAccount, GuildContext, SentMessage};

fn account_input() -> ExportAccount {
    ExportAccount {
        id: DiscordId::parse("9007199254741001").unwrap(),
        username: "invented_owner".into(),
    }
}
fn channel_input() -> ChannelContext {
    ChannelContext {
        id: DiscordId::parse("9007199254741101").unwrap(),
        source_type: "invented_direct".into(),
        name: Some("Invented room".into()),
        recipients: Some(vec!["opaque recipient".into()]),
        guild: None,
    }
}
fn message_input() -> SentMessage {
    SentMessage {
        id: DiscordId::parse("1985931830091579393").unwrap(),
        account_id: account_input().id,
        channel_id: channel_input().id,
        timestamp_millis: 1_893_553_445_123,
        contents: "Invented hello 🌱\nowner@example.test".into(),
        attachments: "https://example.invalid/passport.png?signature=one".into(),
    }
}
fn normalizer(source: u128) -> DiscordNormalizer {
    DiscordNormalizer::new(scope(source), observed()).unwrap()
}
fn content() -> ContentRecord {
    normalizer(2)
        .content(&account_input(), &channel_input(), &message_input())
        .unwrap()
}
fn source_record() -> SourceRecord {
    SourceRecord {
        id: scope(2).source_id,
        account_id: scope(2).account_id,
        provider: discord_provider_key(),
        kind: SourceKind::ArchiveImport,
        state: SourceState::Preparing,
        archive_fingerprint: Some(format!("sha256:{}", "a".repeat(64))),
        schema_profile: DiscordSourceProfile::payload(),
        imported_at: None,
        updated_at: observed(),
        warnings: vec![],
    }
}

fn scope(source: u128) -> Scope {
    Scope {
        provider: "discord".to_owned().try_into().unwrap(),
        account_id: Uuid::from_u128(1).try_into().unwrap(),
        source_id: Uuid::from_u128(source).try_into().unwrap(),
    }
}

fn observed() -> DateTime<Utc> {
    "2031-01-02T03:04:05.123Z".parse().unwrap()
}

#[test]
fn discord_stable_locator_envelopes_and_tuple_keys_preserve_lossless_ids() {
    let user = DiscordUserLocator::new("18446744073709551615").unwrap();
    assert_eq!(
        user.payload(),
        VersionedPayload {
            schema: "discord.user".into(),
            version: 1,
            payload: json!({"userId": "18446744073709551615"}),
        }
    );
    assert_eq!(user.canonical_key(), "18446744073709551615");
    let actor = user.resource(scope(2).account_id);
    assert_eq!(actor.resource_kind, ResourceKind::Actor);
    assert_eq!(actor.locator_schema, "discord.user");
    assert_eq!(
        actor.locator_payload,
        json!({"userId": "18446744073709551615"})
    );
    let channel = DiscordChannelLocator::new("9007199254741101")
        .unwrap()
        .resource(scope(2).account_id);
    assert_eq!(channel.locator_schema, "discord.channel");
    assert_eq!(channel.locator_version, 1);
    assert_eq!(channel.canonical_key, "9007199254741101");
    assert_eq!(
        channel.locator_payload,
        json!({"channelId": "9007199254741101"})
    );
    let message = DiscordMessageLocator::new("9007199254741101", "1985931830091579393").unwrap();
    let resource = message.resource(scope(2).account_id);
    assert_eq!(resource.provider, discord_provider_key());
    assert_eq!(resource.locator_schema, "discord.message");
    assert_eq!(resource.locator_version, 1);
    assert_eq!(
        resource.canonical_key,
        "16:900719925474110119:1985931830091579393"
    );
    assert_eq!(
        resource.locator_payload,
        json!({"channelId": "9007199254741101", "messageId": "1985931830091579393"})
    );
    assert_eq!(
        DiscordMessageLocator::new("1", "23")
            .unwrap()
            .canonical_key(),
        "1:12:23"
    );
    assert_eq!(
        DiscordMessageLocator::new("12", "3")
            .unwrap()
            .canonical_key(),
        "2:121:3"
    );
    assert_ne!(
        DiscordMessageLocator::new("1", "23")
            .unwrap()
            .resource(scope(2).account_id)
            .resource_id()
            .unwrap(),
        DiscordMessageLocator::new("12", "3")
            .unwrap()
            .resource(scope(2).account_id)
            .resource_id()
            .unwrap()
    );
    for resource in [actor, channel, resource] {
        DiscordPayloadValidator
            .validate_resource(&resource)
            .unwrap();
    }
}

#[test]
fn discord_locators_reject_noncanonical_and_out_of_range_identifiers() {
    for invalid in [
        "",
        "0",
        "-0",
        "-1",
        "+1",
        "01",
        " 1",
        "1 ",
        "1.0",
        "1e3",
        "１",
        "18446744073709551616",
    ] {
        assert!(DiscordUserLocator::new(invalid).is_err());
        assert!(DiscordChannelLocator::new(invalid).is_err());
        assert!(DiscordMessageLocator::new("1", invalid).is_err());
        assert!(DiscordMessageLocator::new(invalid, "1").is_err());
    }
}

#[test]
fn discord_validator_rejects_unknown_mutable_and_disagreeing_resource_envelopes() {
    let resource = DiscordMessageLocator::new("1", "23")
        .unwrap()
        .resource(scope(2).account_id);
    for (field, value) in [
        ("locatorSchema", json!("discord.future")),
        ("locatorVersion", json!(2)),
        ("canonicalKey", json!("2:121:3")),
        ("provider", json!("telegram")),
        ("resourceKind", json!("actor")),
        (
            "locatorPayload",
            json!({"channelId": "1", "messageId": "023"}),
        ),
    ] {
        let mut value_resource = serde_json::to_value(&resource).unwrap();
        value_resource[field] = value;
        let changed = serde_json::from_value(value_resource).unwrap();
        assert!(
            DiscordPayloadValidator.validate_resource(&changed).is_err(),
            "{field}"
        );
    }
    for field in [
        "title",
        "guildId",
        "timestamp",
        "url",
        "observedAt",
        "recipe",
    ] {
        let mut changed = resource.clone();
        changed.locator_payload[field] = json!("mutable");
        assert!(
            DiscordPayloadValidator.validate_resource(&changed).is_err(),
            "{field}"
        );
    }
    assert_eq!(
        DiscordPayloadValidator.validation_policy_key().as_str(),
        "discord.archive_payload.v1"
    );
}

#[test]
fn discord_normalizer_rejects_foreign_scope() {
    let mut foreign = scope(2);
    foreign.provider = "telegram".to_owned().try_into().unwrap();
    assert!(DiscordNormalizer::new(foreign, observed()).is_err());
}

#[test]
fn discord_normalization_preserves_observations_and_never_claims_live_evidence() {
    let normalizer = normalizer(2);
    let account = normalizer.account(&account_input()).unwrap();
    assert_eq!(
        account.native_identity.payload,
        json!({"userId": "9007199254741001"})
    );
    assert_eq!(account.native_identity.schema, "discord.user");
    assert_eq!(account.connection_state, ConnectionState::Disconnected);
    assert_eq!(account.display_name, "invented_owner");
    assert_eq!(account.created_at, observed());
    assert_eq!(account.last_seen_at, observed());
    assert!(account.avatar.is_none());
    let actor = normalizer.actor(&account_input()).unwrap();
    assert_eq!(actor.resource.canonical_key, "9007199254741001");
    assert_eq!(actor.display_name, "invented_owner");
    assert_eq!(actor.evidence, EvidenceState::Archive);
    assert_eq!(actor.observed_at, observed());
    let conversation = normalizer.conversation(&channel_input()).unwrap();
    assert_eq!(conversation.kind, ConversationKind::Other);
    assert_eq!(conversation.title, "Invented room");
    assert_eq!(
        conversation.provider_metadata.unwrap().payload,
        json!({
            "sourceType": "invented_direct", "channelName": "Invented room", "verifiedKind": "other", "guild": null,
            "recipients": ["opaque recipient"], "warnings": ["unknown_conversation_kind"],
        })
    );
    assert!(conversation.participants.is_empty());
    assert_eq!(conversation.participant_count, None);
    assert_eq!(conversation.parent_id, None);
    let record = content();
    assert_eq!(record.conversation_id, conversation.id);
    assert_eq!(record.author_id, actor.id);
    assert_eq!(
        record.timestamp,
        "2030-01-02T03:04:05.123Z".parse::<DateTime<Utc>>().unwrap()
    );
    assert_eq!(
        record.searchable_text,
        "Invented hello 🌱\nowner@example.test"
    );
    assert_eq!(record.kind, ContentKind::Text);
    assert_eq!(record.evidence, EvidenceState::Archive);
    assert_eq!(record.observed_at, observed());
    assert_eq!(
        record.external_location,
        ExternalLocationAvailability::Unsupported
    );
    assert!(
        record.edited_at.is_none() && record.reply_to.is_none() && record.thread_parent.is_none()
    );
    assert!(record.privacy_findings.is_empty() && record.detector_version.is_none());
    assert_eq!(record.attachments.len(), 1);
    assert_eq!(record.attachments[0].kind, ContentKind::Other);
    assert_eq!(
        record.attachments[0].safe_display_name.as_deref(),
        Some("passport.png")
    );
    assert!(
        record.attachments[0].size_bytes.is_none() && record.attachments[0].mime_type.is_none()
    );
    assert_eq!(
        record.attachments[0].locator,
        VersionedPayload {
            schema: "discord.attachment".into(),
            version: 1,
            payload: json!({"channelId": "9007199254741101", "messageId": "1985931830091579393", "ordinal": 0}),
        }
    );
    assert_eq!(
        record.provider_metadata.unwrap().payload,
        json!({
            "authorUserId": "9007199254741001", "attachmentUrls": ["https://example.invalid/passport.png?signature=one"],
        })
    );
}

#[test]
fn discord_verified_kinds_are_explicit_and_never_inferred_from_source_strings_or_guilds() {
    for (input, expected) in [
        (VerifiedConversationKind::Direct, ConversationKind::Direct),
        (
            VerifiedConversationKind::GroupDirect,
            ConversationKind::Group,
        ),
        (
            VerifiedConversationKind::GuildChannel,
            ConversationKind::CommunityChannel,
        ),
        (VerifiedConversationKind::Other, ConversationKind::Other),
    ] {
        let record = normalizer(2)
            .conversation_with_kind(&channel_input(), input)
            .unwrap();
        assert_eq!(record.kind, expected);
        DiscordPayloadValidator
            .validate_archive_conversation(&record)
            .unwrap();
    }
    for source_type in [
        "DM",
        "GROUP_DM",
        "GUILD_TEXT",
        "1",
        "3",
        "",
        "invented_group",
    ] {
        let mut channel = channel_input();
        channel.source_type = source_type.into();
        channel.recipients = None;
        channel.guild = Some(GuildContext {
            id: DiscordId::parse("9007199254741301").unwrap(),
            name: "Invented Guild".into(),
        });
        let record = normalizer(2).conversation(&channel).unwrap();
        assert_eq!(record.kind, ConversationKind::Other);
        assert!(
            record.parent_id.is_none()
                && record.participant_count.is_none()
                && record.participants.is_empty()
        );
    }
}

#[test]
fn discord_resource_identity_is_stable_across_sources_and_mutable_observation_changes() {
    let first = normalizer(2);
    let second = DiscordNormalizer::new(scope(3), "2032-01-01T00:00:00Z".parse().unwrap()).unwrap();
    let old_actor = first.actor(&account_input()).unwrap();
    let mut changed_account = account_input();
    changed_account.username = "renamed_owner".into();
    let new_actor = second.actor(&changed_account).unwrap();
    assert_eq!(old_actor.id, new_actor.id);
    assert_eq!(old_actor.resource, new_actor.resource);
    assert_ne!(old_actor, new_actor);
    let mut changed_channel = channel_input();
    changed_channel.name = Some("Renamed room".into());
    changed_channel.recipients = None;
    changed_channel.guild = Some(GuildContext {
        id: DiscordId::parse("9007199254741301").unwrap(),
        name: "Guild enrichment".into(),
    });
    let old_room = first.conversation(&channel_input()).unwrap();
    let new_room = second.conversation(&changed_channel).unwrap();
    assert_eq!(old_room.id, new_room.id);
    assert_eq!(old_room.resource, new_room.resource);
    assert_ne!(old_room, new_room);
    let old = content();
    let mut changed_message = message_input();
    changed_message.contents = "Different observation".into();
    changed_message.timestamp_millis += 1000;
    changed_message.attachments = "https://example.invalid/renamed.png?signature=two".into();
    let new = second
        .content(&changed_account, &changed_channel, &changed_message)
        .unwrap();
    assert_eq!(old.id, new.id);
    assert_eq!(old.resource, new.resource);
    assert_eq!(old.attachments[0].locator, new.attachments[0].locator);
    assert_ne!(old, new);
    assert_eq!(new.scope, scope(3));
    assert_eq!(
        new.timestamp.timestamp_millis(),
        changed_message.timestamp_millis
    );
    assert_eq!(
        new.observed_at,
        "2032-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
    );
}

#[test]
fn discord_normalizer_rejects_identity_context_and_timestamp_mismatch() {
    let mut message = message_input();
    message.account_id = DiscordId::parse("2").unwrap();
    assert!(
        normalizer(2)
            .content(&account_input(), &channel_input(), &message)
            .is_err()
    );
    message = message_input();
    message.channel_id = DiscordId::parse("2").unwrap();
    assert!(
        normalizer(2)
            .content(&account_input(), &channel_input(), &message)
            .is_err()
    );
    message = message_input();
    message.timestamp_millis = i64::MAX;
    assert!(
        normalizer(2)
            .content(&account_input(), &channel_input(), &message)
            .is_err()
    );
}

#[test]
fn discord_attachment_only_and_empty_records_keep_unknown_media_and_exact_text() {
    let mut message = message_input();
    message.contents.clear();
    let record = normalizer(2)
        .content(&account_input(), &channel_input(), &message)
        .unwrap();
    assert_eq!(record.kind, ContentKind::Other);
    assert_eq!(record.attachments[0].kind, ContentKind::Other);
    message.attachments.clear();
    let record = normalizer(2)
        .content(&account_input(), &channel_input(), &message)
        .unwrap();
    assert_eq!(record.kind, ContentKind::Text);
    assert!(record.searchable_text.is_empty() && record.attachments.is_empty());
}

#[test]
fn discord_space_delimited_attachment_exports_preserve_each_url_and_ordinal() {
    let mut message = message_input();
    message.attachments = concat!(
        "https://example.invalid/first%20photo.png?signature=one ",
        "https://cdn.example.invalid/second.pdf?signature=two"
    )
    .into();

    let record = normalizer(2)
        .content(&account_input(), &channel_input(), &message)
        .unwrap();

    assert_eq!(record.attachments.len(), 2);
    assert_eq!(
        record.attachments[0].safe_display_name.as_deref(),
        Some("first photo.png")
    );
    assert_eq!(
        record.attachments[1].safe_display_name.as_deref(),
        Some("second.pdf")
    );
    assert_eq!(record.attachments[0].locator.payload["ordinal"], json!(0));
    assert_eq!(record.attachments[1].locator.payload["ordinal"], json!(1));
    assert_eq!(
        record.provider_metadata.unwrap().payload["attachmentUrls"],
        json!([
            "https://example.invalid/first%20photo.png?signature=one",
            "https://cdn.example.invalid/second.pdf?signature=two"
        ])
    );
}

#[test]
fn discord_attachment_urls_reject_active_local_ambiguous_and_malformed_references() {
    for url in [
        "http://example.invalid/file",
        "file:///tmp/private",
        "javascript:alert(1)",
        "data:text/plain,x",
        "ftp://example.invalid/file",
        "//example.invalid/file",
        "https:///",
        "https://",
        "https:///example.invalid/file",
        "https:////example.invalid/file",
        "https://example.invalid/a\nhttps://example.invalid/b",
        " https://example.invalid/a",
        "https://example.invalid/a\t",
        "https:\\example.invalid\\file",
        "https://user:pass@example.invalid/file",
        "https://example.invalid/%zz",
        "https://example.invalid/%00",
        "https://example.invalid/%ff",
        "https://example.invalid/a%2Fb",
        "https://example.invalid/a%5Cb",
        "https://example.invalid/a,https://example.invalid/b",
    ] {
        let mut message = message_input();
        message.attachments = url.into();
        assert!(
            normalizer(2)
                .content(&account_input(), &channel_input(), &message)
                .is_err(),
            "{url}"
        );
        let mut record = content();
        record.provider_metadata.as_mut().unwrap().payload["attachmentUrls"] = json!([url]);
        assert!(
            DiscordPayloadValidator
                .validate_archive_content(&record)
                .is_err(),
            "{url}"
        );
    }
    let mut message = message_input();
    message.attachments = "https://example.invalid/%70assport.png?signature=two".into();
    let record = normalizer(2)
        .content(&account_input(), &channel_input(), &message)
        .unwrap();
    assert_eq!(
        record.attachments[0].safe_display_name.as_deref(),
        Some("passport.png")
    );
    assert_eq!(
        record.provider_metadata.unwrap().payload["attachmentUrls"][0],
        message.attachments
    );
}

#[test]
fn discord_account_source_and_nested_envelopes_fail_closed() {
    let account = normalizer(2).account(&account_input()).unwrap();
    assert_eq!(
        DiscordPayloadValidator
            .validate_account(&account)
            .unwrap()
            .as_canonical_str(),
        "9007199254741001"
    );
    DiscordPayloadValidator
        .validate_source(&source_record(), &account)
        .unwrap();
    assert_eq!(
        source_record().schema_profile,
        VersionedPayload {
            schema: "discord.data_package.messages_json".into(),
            version: 1,
            payload: json!({"policyKey": "discord.import_policy.v1"})
        }
    );
    for (field, value) in [
        ("provider", json!("telegram")),
        ("connectionState", json!("ready")),
        (
            "avatar",
            json!({"schema":"discord.user", "version":1,"payload":{"userId":"1"}}),
        ),
    ] {
        let mut changed = serde_json::to_value(&account).unwrap();
        changed[field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_account(&serde_json::from_value(changed).unwrap())
                .is_err()
        );
    }
    for (field, value) in [
        ("accountId", json!(Uuid::from_u128(9))),
        ("provider", json!("telegram")),
        ("kind", json!("live_connection")),
    ] {
        let mut changed = serde_json::to_value(source_record()).unwrap();
        changed[field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_source(&serde_json::from_value(changed).unwrap(), &account)
                .is_err()
        );
    }
    for (field, value) in [
        ("schema", json!("discord.future")),
        ("version", json!(2)),
        ("payload", json!({"policyKey":"unknown"})),
        (
            "payload",
            json!({"policyKey":"discord.import_policy.v1", "recipe":{}}),
        ),
    ] {
        let mut changed = serde_json::to_value(source_record()).unwrap();
        changed["schemaProfile"][field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_source(&serde_json::from_value(changed).unwrap(), &account)
                .is_err()
        );
    }
    let mut room = normalizer(2).conversation(&channel_input()).unwrap();
    room.participants
        .push(normalizer(2).actor(&account_input()).unwrap());
    DiscordPayloadValidator
        .validate_archive_conversation(&room)
        .unwrap();
    for (field, value) in [
        ("locatorSchema", json!("discord.future")),
        ("locatorVersion", json!(2)),
        ("canonicalKey", json!("2")),
        ("accountId", json!(Uuid::from_u128(9))),
        ("provider", json!("telegram")),
        ("resourceKind", json!("content")),
        ("locatorPayload", json!({"userId":"1","name":"mutable"})),
    ] {
        let mut changed = serde_json::to_value(&room).unwrap();
        changed["participants"][0]["resource"][field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_archive_conversation(&serde_json::from_value(changed).unwrap())
                .is_err(),
            "nested {field}"
        );
    }
    for field in ["avatar", "evidence", "scope"] {
        let mut changed = serde_json::to_value(&room).unwrap();
        changed["participants"][0][field] = match field {
            "avatar" => json!({"schema":"discord.user","version":1,"payload":{"userId":"1"}}),
            "evidence" => json!("live"),
            _ => serde_json::to_value(scope(3)).unwrap(),
        };
        assert!(
            DiscordPayloadValidator
                .validate_archive_conversation(&serde_json::from_value(changed).unwrap())
                .is_err()
        );
    }
}

#[test]
fn discord_content_validator_checks_every_nested_attachment_and_reference_agreement() {
    let original = content();
    DiscordPayloadValidator
        .validate_archive_content(&original)
        .unwrap();
    for (field, value) in [
        ("schema", json!("discord.future")),
        ("version", json!(2)),
        (
            "payload",
            json!({"channelId":"2","messageId":"1985931830091579393","ordinal":0}),
        ),
        (
            "payload",
            json!({"channelId":"9007199254741101","messageId":"2","ordinal":0}),
        ),
        (
            "payload",
            json!({"channelId":"9007199254741101","messageId":"1985931830091579393","ordinal":1}),
        ),
        (
            "payload",
            json!({"channelId":"9007199254741101","messageId":"1985931830091579393","ordinal":0,"url":"https://example.invalid/a"}),
        ),
    ] {
        let mut changed = serde_json::to_value(&original).unwrap();
        changed["attachments"][0]["locator"][field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_archive_content(&serde_json::from_value(changed).unwrap())
                .is_err()
        );
    }
    for (field, value) in [
        ("evidence", json!("live")),
        ("externalLocation", json!("available")),
        ("conversationId", json!(Uuid::from_u128(9))),
        ("authorId", json!(Uuid::from_u128(9))),
        ("editedAt", json!("2030-01-02T03:04:05Z")),
        ("replyTo", json!(Uuid::from_u128(9))),
        ("threadParent", json!(Uuid::from_u128(9))),
    ] {
        let mut changed = serde_json::to_value(&original).unwrap();
        changed[field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_archive_content(&serde_json::from_value(changed).unwrap())
                .is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("kind", json!("image")),
        ("mimeType", json!("image/png")),
        ("sizeBytes", json!(12)),
        ("safeDisplayName", json!("forged.txt")),
    ] {
        let mut changed = serde_json::to_value(&original).unwrap();
        changed["attachments"][0][field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_archive_content(&serde_json::from_value(changed).unwrap())
                .is_err(),
            "{field}"
        );
    }
    for field in ["recipe", "unknown"] {
        let mut changed = original.clone();
        changed.provider_metadata.as_mut().unwrap().payload[field] = json!({});
        assert!(
            DiscordPayloadValidator
                .validate_archive_content(&changed)
                .is_err()
        );
    }
    let mut changed = original.clone();
    changed.attachments.push(changed.attachments[0].clone());
    assert!(
        DiscordPayloadValidator
            .validate_archive_content(&changed)
            .is_err()
    );
}

#[test]
fn discord_provider_envelopes_and_searchable_input_share_archive_size_ceilings() {
    use crate::persistence::archive::{ENVELOPE_BYTES, MAX_BATCH_RECORDS, MAX_SEARCHABLE_BYTES};
    let mut message = message_input();
    message.attachments.clear();
    message.contents = "x".repeat(MAX_SEARCHABLE_BYTES);
    assert!(
        normalizer(2)
            .content(&account_input(), &channel_input(), &message)
            .is_ok()
    );
    message.contents.push('x');
    assert!(
        normalizer(2)
            .content(&account_input(), &channel_input(), &message)
            .is_err()
    );
    let mut changed = content();
    changed.searchable_text = "x".repeat(MAX_SEARCHABLE_BYTES);
    assert!(
        DiscordPayloadValidator
            .validate_archive_content(&changed)
            .is_err()
    );
    let mut changed = content();
    changed.provider_metadata.as_mut().unwrap().payload["attachmentUrls"] = json!([format!(
        "https://example.invalid/{}",
        "x".repeat(ENVELOPE_BYTES)
    )]);
    assert!(
        DiscordPayloadValidator
            .validate_archive_content(&changed)
            .is_err()
    );
    let mut account = account_input();
    account.username =
        "x".repeat(discord_archive::ArchiveLimits::default().max_display_bytes as usize + 1);
    assert!(normalizer(2).account(&account).is_err());
    assert!(normalizer(2).actor(&account).is_err());
    let mut room = normalizer(2).conversation(&channel_input()).unwrap();
    room.participants = vec![normalizer(2).actor(&account_input()).unwrap(); MAX_BATCH_RECORDS];
    assert!(
        DiscordPayloadValidator
            .validate_archive_conversation(&room)
            .is_err()
    );
}

#[test]
fn discord_rejects_all_executable_recipes_and_job_validation() {
    let plan = RemediationPlan {
        id: Uuid::from_u128(4),
        scope: scope(2),
        steps: vec![],
        targets: vec![],
        confirmation: ConfirmationRequirements {
            tier: ConfirmationTier::Low,
            acknowledgement_required: false,
            owner_auth_required: false,
            exact_text: None,
        },
        recipe: VersionedPayload {
            schema: "discord.remediation_recipe".into(),
            version: 1,
            payload: json!({}),
        },
        restart_policy: RestartPolicy::RequiresNewReview,
        created_at: observed(),
        fingerprint: "".into(),
    };
    assert!(DiscordPayloadValidator.validate_recipe(&plan).is_err());
    let job = ScopedJobRecord {
        id: Uuid::from_u128(5),
        plan_id: plan.id,
        scope: scope(2),
        dirty_refs: vec![],
        status: JobStatus::Queued,
        counters: JobCounters::default(),
        next_batch: 0,
        retry_at: None,
        diagnostics: vec![],
        started_authorized: false,
        created_at: observed(),
        updated_at: observed(),
    };
    assert!(DiscordPayloadValidator.validate_job(&plan, &job).is_err());
}

#[test]
fn discord_optional_channel_names_preserve_source_differences_and_title_agreement() {
    let mut input = channel_input();
    input.name = None;
    let absent = normalizer(2).conversation(&input).unwrap();
    input.name = Some(String::new());
    let empty = normalizer(2).conversation(&input).unwrap();
    assert_eq!(absent.resource, empty.resource);
    assert_ne!(absent, empty);
    let mut changed = normalizer(2).conversation(&channel_input()).unwrap();
    changed.title = "inconsistent observation".into();
    assert!(
        DiscordPayloadValidator
            .validate_archive_conversation(&changed)
            .is_err()
    );
}

#[test]
fn discord_unknown_metadata_schemas_fields_versions_and_identity_spelling_reject() {
    let account = normalizer(2).account(&account_input()).unwrap();
    for (field, value) in [
        ("schema", json!("discord.future")),
        ("version", json!(2)),
        ("payload", json!({"userId":"01"})),
        (
            "payload",
            json!({"userId":"9007199254741001","username":"mutable"}),
        ),
    ] {
        let mut changed = serde_json::to_value(&account).unwrap();
        changed["nativeIdentity"][field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_account(&serde_json::from_value(changed).unwrap())
                .is_err()
        );
    }
    let room = normalizer(2).conversation(&channel_input()).unwrap();
    let record = content();
    for (field, value) in [
        ("schema", json!("discord.future")),
        ("version", json!(2)),
        ("payload", json!({})),
    ] {
        let mut changed = serde_json::to_value(&room).unwrap();
        changed["providerMetadata"][field] = value.clone();
        assert!(
            DiscordPayloadValidator
                .validate_archive_conversation(&serde_json::from_value(changed).unwrap())
                .is_err()
        );
        let mut changed = serde_json::to_value(&record).unwrap();
        changed["providerMetadata"][field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_archive_content(&serde_json::from_value(changed).unwrap())
                .is_err()
        );
    }
    for (field, value) in [
        ("guild", json!({"id":"01","name":"Guild"})),
        ("verifiedKind", json!("guessed")),
        ("warnings", json!(["arbitrary source string"])),
        ("recipe", json!({})),
    ] {
        let mut changed = room.clone();
        changed.provider_metadata.as_mut().unwrap().payload[field] = value;
        assert!(
            DiscordPayloadValidator
                .validate_archive_conversation(&changed)
                .is_err()
        );
    }
    for kind in ["conversation", "content", "actor"] {
        let original = match kind {
            "conversation" => serde_json::to_value(&room).unwrap(),
            "content" => serde_json::to_value(&record).unwrap(),
            _ => serde_json::to_value(normalizer(2).actor(&account_input()).unwrap()).unwrap(),
        };
        for (field, value) in [
            ("accountId", json!(Uuid::from_u128(9))),
            ("provider", json!("telegram")),
            ("resourceKind", json!("grouping")),
        ] {
            let mut changed = original.clone();
            changed["resource"][field] = value;
            assert!(
                match kind {
                    "conversation" => DiscordPayloadValidator
                        .validate_archive_conversation(&serde_json::from_value(changed).unwrap()),
                    "content" => DiscordPayloadValidator
                        .validate_archive_content(&serde_json::from_value(changed).unwrap()),
                    _ => DiscordPayloadValidator
                        .validate_archive_actor(&serde_json::from_value(changed).unwrap()),
                }
                .is_err()
            );
        }
    }
}

#[test]
fn discord_envelope_boundary_matches_the_shared_encoded_byte_limit() {
    use crate::persistence::archive::{ENVELOPE_BYTES, ImportBatch, encoded_size};
    let mut input = channel_input();
    input.recipients = Some(vec!["x".repeat(4000); 16]);
    let base = normalizer(2).conversation(&input).unwrap();
    let bytes = encoded_size(base.provider_metadata.as_ref().unwrap(), ENVELOPE_BYTES).unwrap();
    // Appending an ASCII recipient adds two quotes and one separating comma.
    let remaining = ENVELOPE_BYTES - bytes - 3;
    input
        .recipients
        .as_mut()
        .unwrap()
        .push("x".repeat(remaining));
    let at_limit = normalizer(2).conversation(&input).unwrap();
    assert_eq!(
        encoded_size(at_limit.provider_metadata.as_ref().unwrap(), ENVELOPE_BYTES).unwrap(),
        ENVELOPE_BYTES
    );
    assert!(
        ImportBatch {
            conversations: vec![at_limit.clone()],
            ..Default::default()
        }
        .bounded_size()
        .is_ok()
    );
    let mut too_large = at_limit;
    too_large.provider_metadata.as_mut().unwrap().payload["recipients"][16] =
        json!("x".repeat(remaining + 1));
    assert!(
        DiscordPayloadValidator
            .validate_archive_conversation(&too_large)
            .is_err()
    );
    assert!(
        ImportBatch {
            conversations: vec![too_large],
            ..Default::default()
        }
        .bounded_size()
        .is_err()
    );
    input.recipients.as_mut().unwrap()[16].push('x');
    assert!(normalizer(2).conversation(&input).is_err());
}
