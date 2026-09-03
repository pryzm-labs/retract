use chrono::{DateTime, Utc};
use cleaner_domain::{
    ChatKind, ChatSummary, ConfirmationTier, ContentKind, DeletionPlan, MessageSnapshot,
    PlanOperation, PlanSummary,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub runtime_mode: String,
    pub account_label: String,
    pub mode_reason: Option<String>,
    pub chats: Vec<ChatSummary>,
    pub recent_jobs: Vec<JobRecord>,
    pub safety_notice: String,
    pub auth: AuthSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogProgress {
    pub phase: &'static str,
    pub total: usize,
    pub processed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStage {
    Initializing,
    WaitingForPhone,
    WaitingForEmailAddress,
    WaitingForEmailCode,
    WaitingForCode,
    WaitingForPassword,
    WaitingForOtherDevice,
    Ready,
    LoggingOut,
    Closed,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSnapshot {
    pub stage: AuthStage,
    pub hint: Option<String>,
    pub qr_link: Option<String>,
}

impl AuthSnapshot {
    #[cfg(test)]
    pub fn ready() -> Self {
        Self {
            stage: AuthStage::Ready,
            hint: None,
            qr_link: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    #[default]
    Any,
    Mine,
    Others,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub chat_ids: Vec<i64>,
    #[serde(default)]
    pub chat_kinds: Vec<ChatKind>,
    #[serde(default)]
    pub content_kinds: Vec<ContentKind>,
    #[serde(default)]
    pub direction: MessageDirection,
    pub min_date: Option<DateTime<Utc>>,
    pub max_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub exclude_pinned: bool,
    #[serde(default)]
    pub privacy_scan: bool,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    500
}

impl SearchRequest {
    pub fn validate(&mut self) -> Result<(), crate::error::AppError> {
        self.query = self.query.trim().chars().take(256).collect();
        if self.limit == 0 || self.limit > 10_000 {
            return Err(crate::error::AppError::InvalidRequest(
                "search limit must be between 1 and 10,000".into(),
            ));
        }
        if self.chat_ids.len() > 1_000 {
            return Err(crate::error::AppError::InvalidRequest(
                "too many chat filters".into(),
            ));
        }
        if let (Some(min), Some(max)) = (self.min_date, self.max_date)
            && min > max
        {
            return Err(crate::error::AppError::InvalidRequest(
                "the start date must not be after the end date".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub messages: Vec<MessageSnapshot>,
    pub returned: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareSelectionRequest {
    pub message_refs: Vec<MessageRef>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    pub chat_id: i64,
    pub message_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareChatActionRequest {
    pub chat_id: i64,
    pub operation: PlanOperation,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareSenderActionRequest {
    pub chat_id: i64,
    pub sender_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthValueRequest {
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub id: Uuid,
    pub operation: PlanOperation,
    pub chat_title: Option<String>,
    pub target_sender_name: Option<String>,
    pub summary: PlanSummary,
    pub confirmation_tier: ConfirmationTier,
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
}

impl From<&DeletionPlan> for PlanView {
    fn from(value: &DeletionPlan) -> Self {
        Self {
            id: value.id,
            operation: value.operation,
            chat_title: value.chat_title.clone(),
            target_sender_name: value.target_sender_name.clone(),
            summary: value.summary.clone(),
            confirmation_tier: value.confirmation_tier,
            fingerprint: value.fingerprint.clone(),
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteRequest {
    pub plan_id: Uuid,
    pub fingerprint: String,
    pub irreversible_acknowledged: bool,
    pub typed_chat_title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizePlanRequest {
    pub plan_id: Uuid,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Partial,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Partial | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub operation: PlanOperation,
    #[serde(default)]
    pub target_chat_ids: Vec<i64>,
    pub status: JobStatus,
    pub total: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub failed: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub uncertain: usize,
    #[serde(default)]
    pub next_batch: usize,
    #[serde(default)]
    pub retry_after_seconds: Option<u64>,
    pub error_codes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl JobRecord {
    pub fn new(plan: &DeletionPlan) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            plan_id: plan.id,
            operation: plan.operation,
            target_chat_ids: affected_chat_ids(plan),
            status: JobStatus::Queued,
            total: plan.summary.delete_for_everyone,
            deleted: 0,
            skipped: plan.summary.self_only + plan.summary.cannot_delete,
            failed: 0,
            uncertain: 0,
            next_batch: 0,
            retry_after_seconds: None,
            error_codes: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub(crate) fn backfill_target_chat_ids(&mut self, plan: &DeletionPlan) {
        if self.target_chat_ids.is_empty() {
            self.target_chat_ids = affected_chat_ids(plan);
        }
    }
}

fn affected_chat_ids(plan: &DeletionPlan) -> Vec<i64> {
    let mut chat_ids = plan
        .target_chat_id
        .into_iter()
        .chain(
            plan.items
                .iter()
                .filter(|item| item.expected_reach == cleaner_domain::DeletionReach::Everyone)
                .map(|item| item.chat_id),
        )
        .collect::<Vec<_>>();
    chat_ids.sort_unstable();
    chat_ids.dedup();
    chat_ids
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedState {
    pub plans: Vec<DeletionPlan>,
    pub jobs: Vec<JobRecord>,
}

#[cfg(test)]
mod wire_contract_tests {
    use super::*;
    use serde_json::Value;

    fn contract() -> Value {
        serde_json::from_str(include_str!(
            "../../src/test/fixtures/telegram-ipc-contract.json"
        ))
        .expect("valid synthetic Telegram IPC contract fixture")
    }

    #[test]
    fn telegram_search_request_wire_names_are_frozen() {
        let mut request: SearchRequest =
            serde_json::from_value(contract()["searchRequest"].clone()).unwrap();
        request.validate().unwrap();
        assert_eq!(request.query, "passport apartment");
        assert_eq!(request.chat_ids, vec![-1001]);
        assert_eq!(request.chat_kinds, vec![ChatKind::Supergroup]);
        assert_eq!(
            request.content_kinds,
            vec![ContentKind::Photo, ContentKind::File]
        );
        assert!(matches!(request.direction, MessageDirection::Mine));
        assert_eq!(request.min_date, Some(instant("2026-08-01T00:00:00Z")));
        assert_eq!(request.max_date, Some(instant("2026-08-31T23:59:59Z")));
        assert_eq!(request.limit, 500);
        assert!(request.exclude_pinned);
        assert!(request.privacy_scan);
    }

    #[test]
    fn telegram_search_request_optional_and_default_boundaries_are_frozen() {
        let mut value = contract()["searchRequest"].clone();
        let object = value.as_object_mut().unwrap();
        for field in [
            "query",
            "chatIds",
            "chatKinds",
            "contentKinds",
            "direction",
            "minDate",
            "maxDate",
            "excludePinned",
            "privacyScan",
            "limit",
        ] {
            object.remove(field);
        }
        let request: SearchRequest = serde_json::from_value(value).unwrap();
        assert!(request.query.is_empty());
        assert!(request.chat_ids.is_empty());
        assert!(request.chat_kinds.is_empty());
        assert!(request.content_kinds.is_empty());
        assert!(matches!(request.direction, MessageDirection::Any));
        assert!(request.min_date.is_none());
        assert!(request.max_date.is_none());
        assert!(!request.exclude_pinned);
        assert!(!request.privacy_scan);
        assert_eq!(request.limit, 500);

        for (wire, expected) in [
            ("any", MessageDirection::Any),
            ("mine", MessageDirection::Mine),
            ("others", MessageDirection::Others),
        ] {
            let direction: MessageDirection = serde_json::from_value(Value::from(wire)).unwrap();
            assert!(std::mem::discriminant(&direction) == std::mem::discriminant(&expected));
        }
    }

    fn instant(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn telegram_response_wire_names_are_frozen() {
        let progress = CatalogProgress {
            phase: "loading",
            total: 531,
            processed: 128,
        };
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            contract()["catalogProgress"]
        );

        let auth = AuthSnapshot {
            stage: AuthStage::WaitingForPassword,
            hint: Some("synthetic hint".into()),
            qr_link: None,
        };
        assert_eq!(
            serde_json::to_value(auth).unwrap(),
            contract()["authSnapshot"]
        );

        let message = MessageSnapshot {
            chat_id: -1001,
            message_id: 14,
            sender_id: 42,
            sender_name: "Synthetic user".into(),
            sent_at: instant("2026-08-15T18:00:00Z"),
            is_outgoing: true,
            content_kind: ContentKind::Photo,
            preview: "Synthetic passport reminder".into(),
            privacy_findings: vec![cleaner_domain::SensitiveDataKind::IdentityDocument],
            album_id: Some(7001),
            is_pinned: false,
            deletion_reach: cleaner_domain::DeletionReach::Everyone,
        };
        let response = SearchResponse {
            messages: vec![message],
            returned: 1,
            truncated: false,
        };
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            contract()["searchResponse"]
        );

        let plan = PlanView {
            id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            operation: PlanOperation::SelectedMessages,
            chat_title: None,
            target_sender_name: None,
            summary: PlanSummary {
                selected: 1,
                delete_for_everyone: 1,
                self_only: 0,
                cannot_delete: 0,
            },
            confirmation_tier: ConfirmationTier::Low,
            fingerprint: "a".repeat(64),
            created_at: instant("2026-08-15T18:01:00Z"),
        };
        assert_eq!(serde_json::to_value(plan).unwrap(), contract()["planView"]);

        let job = JobRecord {
            id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            plan_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            operation: PlanOperation::SelectedMessages,
            target_chat_ids: vec![-1001],
            status: JobStatus::Completed,
            total: 1,
            deleted: 1,
            skipped: 0,
            failed: 0,
            uncertain: 0,
            next_batch: 1,
            retry_after_seconds: None,
            error_codes: Vec::new(),
            created_at: instant("2026-08-15T18:01:01Z"),
            updated_at: instant("2026-08-15T18:01:02Z"),
        };
        assert_eq!(serde_json::to_value(job).unwrap(), contract()["jobRecord"]);
    }
}
