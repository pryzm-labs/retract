//! Authenticated v1 UUID projection regressions; every input is synthetic.

use rusqlite::{Connection, params};

use super::{frozen_v1, migration_validators};
use crate::persistence::archive::{
    ArchiveStore,
    codec::{open_immutable_keyed, open_keyed},
    migration_validation, model, schema,
    test_support::{Fixture, account, key, resource, snapshot_recovery_files, source},
};

fn reject(fixture: &Fixture) {
    let before = snapshot_recovery_files(&fixture.path);
    let db = open_immutable_keyed(&fixture.path, &key()).unwrap();
    schema::validate_v1(&db).unwrap();
    // Independently exercise semantic validation, so a candidate INSERT trigger
    // cannot accidentally be the only protection for known canonical parents.
    assert!(migration_validation::validate(&db, &migration_validators()).is_err());
    drop(db);
    assert!(ArchiveStore::open(fixture.path.clone(), key(), migration_validators()).is_err());
    assert_eq!(snapshot_recovery_files(&fixture.path), before);
    schema::validate_v1(&open_immutable_keyed(&fixture.path, &key()).unwrap()).unwrap();
}

fn parent_corruption(target: &str, urn: bool, foreign_account: bool) {
    let (fixture, _) = frozen_v1();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    let id: String = if foreign_account {
        let mut other = account();
        other.id = uuid::Uuid::from_u128(701).try_into().unwrap();
        other.native_identity.payload = serde_json::json!({"nativeId":"701", "encoding":"decimal"});
        db.execute(
            "INSERT INTO accounts VALUES(?1,?2,?3,?4)",
            params![
                other.provider.as_str(),
                other.id.as_uuid().to_string(),
                "synthetic:701",
                model::encode(&other).unwrap()
            ],
        )
        .unwrap();
        insert_parent_identity(&db, other.id)
    } else {
        db.query_row(target, [], |row| row.get(0)).unwrap()
    };
    // A synthetic authenticated corruption fixture may bypass a write trigger,
    // but restore its exact DDL before opening through production migration.
    let ddl: String = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='conversation_parent_scope_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    db.execute_batch("DROP TRIGGER conversation_parent_scope_update")
        .unwrap();
    db.execute("UPDATE conversation_observations SET record_json=json_set(record_json,'$.parentId',?1) WHERE provider='synthetic'", [if urn { format!("urn:uuid:{id}") } else { id }]).unwrap();
    db.execute_batch(&ddl).unwrap();
    drop(db);
    reject(&fixture);
}

fn insert_parent_identity(db: &Connection, account_id: retract_domain::AccountId) -> String {
    let mut parent = resource("synthetic-parent");
    parent.account_id = account_id;
    parent.resource_kind = retract_domain::ResourceKind::Conversation;
    let id = parent.resource_id().unwrap().to_string();
    db.execute(
        "INSERT INTO resource_identities VALUES(?1,?2,?3,'conversation',?4,?5,?6,?7)",
        params![
            parent.provider.as_str(),
            account_id.as_uuid().to_string(),
            id,
            parent.locator_schema,
            parent.locator_version,
            parent.canonical_key,
            model::encode(&parent).unwrap()
        ],
    )
    .unwrap();
    id
}

const FOREIGN: &str = "SELECT resource_id FROM conversation_observations WHERE provider='discord'";
const SAME_SCOPE: &str =
    "SELECT resource_id FROM conversation_observations WHERE provider='synthetic'";
const WRONG_KIND: &str = "SELECT resource_id FROM actor_observations WHERE provider='synthetic'";

#[test]
fn urn_parent_cannot_resolve_to_foreign_provider() {
    parent_corruption(FOREIGN, true, false);
}
#[test]
fn urn_parent_cannot_resolve_to_foreign_account() {
    parent_corruption("", true, true);
}
#[test]
fn urn_parent_cannot_resolve_to_wrong_kind() {
    parent_corruption(WRONG_KIND, true, false);
}
#[test]
fn urn_parent_is_rejected_even_when_same_scope_and_kind() {
    parent_corruption(SAME_SCOPE, true, false);
}
#[test]
fn semantic_parent_guard_rejects_canonical_foreign_provider() {
    parent_corruption(FOREIGN, false, false);
}
#[test]
fn semantic_parent_guard_rejects_canonical_foreign_account() {
    parent_corruption("", false, true);
}
#[test]
fn semantic_parent_guard_rejects_canonical_wrong_kind() {
    parent_corruption(WRONG_KIND, false, false);
}

fn urn_projection(table: &str, path: &str) {
    let (fixture, _) = frozen_v1();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute(&format!("UPDATE {table} SET record_json=json_set(record_json,?1,'urn:uuid:'||json_extract(record_json,?1)) WHERE provider='synthetic'"), [path]).unwrap();
    drop(db);
    reject(&fixture);
}

macro_rules! projection {
    ($name:ident, $table:literal, $path:literal) => {
        #[test]
        fn $name() {
            urn_projection($table, $path);
        }
    };
}
projection!(actor_id_text_is_canonical, "actor_observations", "$.id");
projection!(
    actor_account_text_is_canonical,
    "actor_observations",
    "$.scope.accountId"
);
projection!(
    actor_source_text_is_canonical,
    "actor_observations",
    "$.scope.sourceId"
);
projection!(
    actor_resource_account_text_is_canonical,
    "actor_observations",
    "$.resource.accountId"
);
projection!(
    conversation_id_text_is_canonical,
    "conversation_observations",
    "$.id"
);
projection!(
    conversation_account_text_is_canonical,
    "conversation_observations",
    "$.scope.accountId"
);
projection!(
    conversation_source_text_is_canonical,
    "conversation_observations",
    "$.scope.sourceId"
);
projection!(
    conversation_resource_account_text_is_canonical,
    "conversation_observations",
    "$.resource.accountId"
);
projection!(content_id_text_is_canonical, "content_observations", "$.id");
projection!(
    content_account_text_is_canonical,
    "content_observations",
    "$.scope.accountId"
);
projection!(
    content_source_text_is_canonical,
    "content_observations",
    "$.scope.sourceId"
);
projection!(
    content_resource_account_text_is_canonical,
    "content_observations",
    "$.resource.accountId"
);
projection!(
    content_conversation_text_is_canonical,
    "content_observations",
    "$.conversationId"
);
projection!(
    content_author_text_is_canonical,
    "content_observations",
    "$.authorId"
);

#[test]
fn optional_content_reference_text_is_canonical() {
    for (field, column, target) in [
        ("replyTo", "reply_to_id", "resource_id"),
        ("threadParent", "thread_parent_id", "conversation_id"),
    ] {
        let (fixture, _) = frozen_v1();
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute(&format!("UPDATE content_observations SET {column}={target},record_json=json_set(record_json,?1,'urn:uuid:'||{target}) WHERE provider='synthetic'"), [format!("$.{field}")]).unwrap();
        drop(db);
        reject(&fixture);
    }
}

#[test]
fn nested_participant_reference_text_is_canonical() {
    for field in [
        "id",
        "scope.accountId",
        "scope.sourceId",
        "resource.accountId",
    ] {
        let (fixture, _) = frozen_v1();
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute_batch("UPDATE conversation_observations SET record_json=json_set(record_json,'$.participants',json_array(json((SELECT record_json FROM actor_observations WHERE provider='synthetic')))) WHERE provider='synthetic'").unwrap();
        let path = format!("$.participants[0].{field}");
        db.execute("UPDATE conversation_observations SET record_json=json_set(record_json,?1,'urn:uuid:'||json_extract(record_json,?1)) WHERE provider='synthetic'", [path]).unwrap();
        drop(db);
        reject(&fixture);
    }
}

#[test]
fn catalog_account_reference_text_is_canonical() {
    let (fixture, _) = frozen_v1();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    db.execute_batch("UPDATE resource_identities SET locator_json=json_set(locator_json,'$.accountId','urn:uuid:'||json_extract(locator_json,'$.accountId')) WHERE provider='synthetic'").unwrap();
    drop(db);
    reject(&fixture);
}

#[test]
fn absent_and_unknown_optional_parents_remain_allowed() {
    for parent in [
        None,
        Some("00000000-0000-0000-0000-000000000799"),
        Some("87545ba2-51ed-5dcc-9960-c58e944cb3a1"),
    ] {
        let (fixture, _) = frozen_v1();
        let db = open_keyed(&fixture.path, &key(), false).unwrap();
        db.execute("UPDATE conversation_observations SET record_json=json_set(record_json,'$.parentId',?1) WHERE provider='synthetic'", [parent]).unwrap();
        drop(db);
        let store = ArchiveStore::open(fixture.path, key(), migration_validators()).unwrap();
        store.transaction(|tx| {
            let retained: Option<String> = tx.query_row("SELECT json_extract(record_json,'$.parentId') FROM conversation_observations WHERE provider='synthetic'", [], |row| row.get(0)).unwrap();
            assert_eq!(retained.as_deref(), parent);
            Ok(())
        }).unwrap();
    }
}

#[test]
fn canonical_parent_retained_after_another_snapshot_is_removed_remains_allowed() {
    let (fixture, _) = frozen_v1();
    let db = open_keyed(&fixture.path, &key(), false).unwrap();
    let id = insert_parent_identity(&db, account().id);
    let mut other = source();
    other.id = uuid::Uuid::from_u128(801).try_into().unwrap();
    db.execute(
        "INSERT INTO sources VALUES(?1,?2,?3,?4)",
        params![
            other.provider.as_str(),
            other.account_id.as_uuid().to_string(),
            other.id.as_uuid().to_string(),
            model::encode(&other).unwrap()
        ],
    )
    .unwrap();
    let mut parent = crate::persistence::archive::test_support::batch("unused", "synthetic")
        .conversations
        .remove(0);
    parent.scope = other.scope();
    parent.resource = model::decode(
        &db.query_row::<String, _, _>(
            "SELECT locator_json FROM resource_identities WHERE resource_id=?",
            [&id],
            |row| row.get(0),
        )
        .unwrap(),
    )
    .unwrap();
    parent.id = parent.resource.resource_id().unwrap().try_into().unwrap();
    db.execute("INSERT INTO conversation_observations(provider,account_id,source_id,resource_id,record_json) VALUES(?1,?2,?3,?4,?5)", params![other.provider.as_str(), other.account_id.as_uuid().to_string(), other.id.as_uuid().to_string(), id, model::encode(&parent).unwrap()]).unwrap();
    db.execute("UPDATE conversation_observations SET record_json=json_set(record_json,'$.parentId',?1) WHERE source_id=?2", params![id, source().id.as_uuid().to_string()]).unwrap();
    db.execute(
        "INSERT INTO cleanup_tasks VALUES(?1,?2,?3,?4,0,'completed')",
        params![
            uuid::Uuid::from_u128(802).to_string(),
            other.provider.as_str(),
            other.account_id.as_uuid().to_string(),
            other.id.as_uuid().to_string()
        ],
    )
    .unwrap();
    db.execute(
        "DELETE FROM sources WHERE source_id=?",
        [other.id.as_uuid().to_string()],
    )
    .unwrap();
    drop(db);
    let store = ArchiveStore::open(fixture.path, key(), migration_validators()).unwrap();
    store.transaction(|tx| {
        assert_eq!(tx.query_row::<String,_,_>("SELECT json_extract(record_json,'$.parentId') FROM conversation_observations WHERE provider='synthetic'", [], |row| row.get(0)).unwrap(), id);
        assert_eq!(tx.query_row::<i64,_,_>("SELECT count(*) FROM conversation_observations WHERE resource_id=?", [&id], |row| row.get(0)).unwrap(), 0);
        assert_eq!(tx.query_row::<i64,_,_>("SELECT count(*) FROM resource_identities WHERE resource_id=?", [&id], |row| row.get(0)).unwrap(), 1);
        Ok(())
    }).unwrap();
}
