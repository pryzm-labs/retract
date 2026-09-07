use super::*;
impl TelegramCleanup {
    pub(crate) async fn start_execution(
        &self,
        request: ExecuteRequest,
    ) -> Result<JobRecord, AppError> {
        self.check_context()?;
        let plan = self
            .plans
            .read()
            .await
            .get(&request.plan_id)
            .cloned()
            .ok_or(AppError::NotFound)?;
        plan.verify_confirmation(&ConfirmationProof {
            fingerprint: request.fingerprint,
            irreversible_acknowledged: request.irreversible_acknowledged,
            typed_chat_title: request.typed_chat_title,
        })?;

        let mut grants = self.system_grants.lock().await;
        self.check_context()?;
        if !grants.consume(
            plan.id,
            &plan.fingerprint,
            self.context.as_ref().map(|c| c.active()),
        ) {
            return Err(AppError::SystemAuthentication(
                "the plan-bound authentication grant is invalid or expired".into(),
            ));
        }
        drop(grants);

        let service = self
            .worker_owner
            .upgrade()
            .ok_or(AppError::StateUnavailable)?;
        let job = JobRecord::new(&plan);
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut running = self.cancellation.lock().await;
        self.transition(|_, jobs| {
            self.check_context()?;
            if jobs.values().any(|existing| existing.plan_id == plan.id) {
                return Err(AppError::InvalidRequest(
                    "this frozen plan has already been started".into(),
                ));
            }
            jobs.insert(job.id, job.clone());
            Ok(())
        })
        .await?;
        running.insert(job.id, cancellation);
        drop(running);

        tauri::async_runtime::spawn(async move {
            service.run_job(job.id).await;
        });
        Ok(job)
    }

    pub(crate) async fn resume_incomplete(&self) {
        // Constructor-time restart policy may have stopped non-idempotent
        // broad jobs. Seal that decision before any safe frozen-ID job resumes.
        if self.check_context().is_err() || self.persist().await.is_err() {
            return;
        }
        let ids: Vec<_> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|job| matches!(job.status, JobStatus::Queued | JobStatus::Running))
            .map(|job| job.id)
            .collect();
        for id in ids {
            let mut running = self.cancellation.lock().await;
            if running.contains_key(&id) {
                continue;
            }
            let Some(service) = self.worker_owner.upgrade() else {
                return;
            };
            running.insert(id, Arc::new(AtomicBool::new(false)));
            drop(running);
            tauri::async_runtime::spawn(async move {
                service.run_job(id).await;
            });
        }
    }

    pub(super) async fn run_job(&self, job_id: Uuid) {
        if let Err(error) = self.run_job_inner(job_id).await {
            // A persistence failure has already quarantined this executor; the
            // committed snapshot remains authoritative and no second save is attempted.
            if !self.persistence_failed.load(Ordering::Acquire)
                && self
                    .transition(|_, jobs| {
                        if let Some(job) = jobs.get_mut(&job_id) {
                            job.status = if job.deleted > 0 {
                                JobStatus::Partial
                            } else {
                                JobStatus::Failed
                            };
                            job.clear_retry();
                            job.updated_at = Utc::now();
                            push_error_once(job, error_code(&error));
                        }
                        Ok(())
                    })
                    .await
                    .is_err()
            {
                self.persistence_failed.store(true, Ordering::Release);
            }
        }
        self.cancellation.lock().await.remove(&job_id);
    }

    pub(super) async fn run_job_inner(&self, job_id: Uuid) -> Result<(), AppError> {
        self.check_context()?;
        let cancellation = self
            .cancellation
            .lock()
            .await
            .get(&job_id)
            .cloned()
            .ok_or(AppError::StateUnavailable)?;
        let retry_at = self
            .jobs
            .read()
            .await
            .get(&job_id)
            .and_then(|job| job.retry_at);
        if let Some(deadline) = retry_at
            && self
                .wait_until_retry(job_id, &cancellation, deadline)
                .await?
        {
            return Ok(());
        }
        let plan_id = self
            .transition(|_, jobs| {
                let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
                if job.status.is_terminal() {
                    return Err(AppError::JobAlreadyTerminal);
                }
                job.status = JobStatus::Running;
                job.updated_at = Utc::now();
                Ok(job.plan_id)
            })
            .await?;

        let plan = self
            .plans
            .read()
            .await
            .get(&plan_id)
            .cloned()
            .ok_or(AppError::NotFound)?;

        if plan.operation == PlanOperation::ClearHistoryAndLeave
            && self
                .run_clear_history_and_leave(job_id, &plan, &cancellation)
                .await?
        {
            return Ok(());
        }

        let deletes_frozen_messages = matches!(
            plan.operation,
            PlanOperation::SelectedMessages
                | PlanOperation::DeleteMyMessages
                | PlanOperation::DeleteAllMessagesAndLeave
                | PlanOperation::LeaveChat
        );
        if deletes_frozen_messages
            && self
                .run_message_batches(job_id, &plan, &cancellation)
                .await?
        {
            return Ok(());
        }

        if !matches!(
            plan.operation,
            PlanOperation::SelectedMessages
                | PlanOperation::DeleteMyMessages
                | PlanOperation::ClearHistoryAndLeave
        ) {
            let operation = plan.operation;
            loop {
                if cancellation.load(Ordering::Acquire) {
                    self.finish_cancelled(job_id).await?;
                    return Ok(());
                }
                let chat_id = if matches!(
                    operation,
                    PlanOperation::DeleteAllMessagesAndLeave | PlanOperation::LeaveChat
                ) {
                    plan.target_chat_id.ok_or_else(|| {
                        AppError::InvalidRequest(
                            "leave plan is missing its immutable chat ID".into(),
                        )
                    })?
                } else {
                    self.resolve_plan_chat(&plan).await?
                };
                if self.stop_if_cancelled(job_id, &cancellation).await? {
                    return Ok(());
                }
                let result = match operation {
                    PlanOperation::ClearHistory => {
                        self.mutation.clear_history_for_everyone(chat_id).await
                    }
                    PlanOperation::RemoveChatForSelf => {
                        self.mutation.remove_chat_for_self(chat_id).await
                    }
                    PlanOperation::DeleteGroup => self.mutation.delete_group(chat_id).await,
                    PlanOperation::DeleteAllMessagesAndLeave | PlanOperation::LeaveChat => {
                        match self
                            .leave_and_remove_chat(job_id, chat_id, &cancellation)
                            .await
                        {
                            Ok(true) => return Ok(()),
                            Ok(false) => Ok(()),
                            Err(error) => Err(error),
                        }
                    }
                    PlanOperation::DeleteBySender => {
                        let sender_id = plan.target_sender_id.ok_or_else(|| {
                            AppError::InvalidRequest(
                                "sender-scoped plan is missing its sender ID".into(),
                            )
                        })?;
                        self.mutation
                            .delete_messages_by_sender(chat_id, sender_id)
                            .await
                    }
                    PlanOperation::SelectedMessages
                    | PlanOperation::DeleteMyMessages
                    | PlanOperation::ClearHistoryAndLeave => {
                        unreachable!()
                    }
                };
                match result {
                    Ok(()) => break,
                    Err(error) => {
                        if let Some(seconds) = telegram_retry_after(&error) {
                            if self.wait_for_retry(job_id, &cancellation, seconds).await? {
                                return Ok(());
                            }
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
        }

        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = if job.failed > 0 {
                JobStatus::Partial
            } else {
                JobStatus::Completed
            };
            job.clear_retry();
            job.updated_at = Utc::now();
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Execute only the everyone-deletable portion of a frozen plan. Returning
    /// `true` means cancellation was recorded and any following chat action
    /// (notably leaving) must not run.
    pub(super) async fn run_message_batches(
        &self,
        job_id: Uuid,
        plan: &DeletionPlan,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        let batches = plan.everyone_batches(100)?;
        let next = self
            .jobs
            .read()
            .await
            .get(&job_id)
            .map(|j| j.next_batch)
            .unwrap_or_default();
        crate::providers::lifecycle::run_frozen_batches(
            &TelegramFrozenDriver {
                service: self,
                job_id,
                cancellation,
            },
            batches,
            next,
        )
        .await
    }

    pub(super) async fn run_clear_history_and_leave(
        &self,
        job_id: Uuid,
        plan: &DeletionPlan,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        let next_phase = self
            .jobs
            .read()
            .await
            .get(&job_id)
            .map(|job| job.next_batch)
            .unwrap_or_default();
        if next_phase == 0 {
            loop {
                if cancellation.load(Ordering::Acquire) {
                    self.finish_cancelled(job_id).await?;
                    return Ok(true);
                }
                let chat_id = self.resolve_plan_chat(plan).await?;
                if self.stop_if_cancelled(job_id, cancellation).await? {
                    return Ok(true);
                }
                match self
                    .mutation
                    .clear_history_for_everyone_keep_chat(chat_id)
                    .await
                {
                    Ok(()) => {
                        self.transition(|_, jobs| {
                            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
                            job.next_batch = 1;
                            job.updated_at = Utc::now();
                            Ok(())
                        })
                        .await?;
                        break;
                    }
                    Err(error) => {
                        if let Some(seconds) = telegram_retry_after(&error) {
                            if self.wait_for_retry(job_id, cancellation, seconds).await? {
                                return Ok(true);
                            }
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
        }

        loop {
            if cancellation.load(Ordering::Acquire) {
                self.finish_cancelled(job_id).await?;
                return Ok(true);
            }
            let chat_id = plan.target_chat_id.ok_or_else(|| {
                AppError::InvalidRequest("leave plan is missing its immutable chat ID".into())
            })?;
            match self
                .leave_and_remove_chat(job_id, chat_id, cancellation)
                .await
            {
                Ok(cancelled) => return Ok(cancelled),
                Err(error) => {
                    if let Some(seconds) = telegram_retry_after(&error) {
                        if self.wait_for_retry(job_id, cancellation, seconds).await? {
                            return Ok(true);
                        }
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    pub(super) async fn leave_and_remove_chat(
        &self,
        job_id: Uuid,
        chat_id: i64,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        let current = self
            .lookup_chat_with_timeout(chat_id, "leave-and-remove state check")
            .await?;
        if self.stop_if_cancelled(job_id, cancellation).await? {
            return Ok(true);
        }

        let Some(current) = current else {
            return Ok(false);
        };

        let refreshed = if current.capabilities.can_leave_chat {
            self.mutation.leave_chat(chat_id).await?;
            // Membership removal is an acknowledged compound step. A failed
            // required save here must prevent the following self-removal call.
            self.transition(|_, jobs| {
                jobs.get_mut(&job_id).ok_or(AppError::NotFound)?.updated_at = Utc::now();
                Ok(())
            })
            .await?;
            if self.stop_if_cancelled(job_id, cancellation).await? {
                return Ok(true);
            }

            self.lookup_chat_with_timeout(chat_id, "post-leave cleanup check")
                .await?
        } else if current.capabilities.can_remove_for_self {
            Some(current)
        } else {
            return Err(AppError::Gateway("CHAT_MEMBER_REQUIRED".into()));
        };

        if self.stop_if_cancelled(job_id, cancellation).await? {
            return Ok(true);
        }

        match refreshed {
            None => Ok(false),
            Some(chat) if chat.capabilities.can_remove_for_self => {
                self.mutation.remove_chat_for_self(chat_id).await?;
                Ok(false)
            }
            Some(_) => Err(AppError::Gateway("CHAT_DELETE_FOR_SELF_FORBIDDEN".into())),
        }
    }

    pub(super) async fn finish_cancelled(&self, job_id: Uuid) -> Result<(), AppError> {
        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Cancelled;
            job.clear_retry();
            job.updated_at = Utc::now();
            Ok(())
        })
        .await
    }

    pub(super) async fn stop_if_cancelled(
        &self,
        job_id: Uuid,
        cancellation: &AtomicBool,
    ) -> Result<bool, AppError> {
        if !cancellation.load(Ordering::Acquire) {
            return Ok(false);
        }
        self.finish_cancelled(job_id).await?;
        Ok(true)
    }

    pub(super) async fn wait_for_retry(
        &self,
        job_id: Uuid,
        cancellation: &AtomicBool,
        seconds: u64,
    ) -> Result<bool, AppError> {
        let deadline = Utc::now() + chrono::Duration::seconds(seconds.min(86400) as i64);
        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Queued;
            job.retry_after_seconds = Some(seconds);
            job.retry_at = Some(deadline);
            job.updated_at = Utc::now();
            push_error_once(job, "telegram_rate_limited");
            Ok(())
        })
        .await?;
        self.wait_until_retry(job_id, cancellation, deadline).await
    }

    pub(super) async fn wait_until_retry(
        &self,
        job_id: Uuid,
        cancellation: &AtomicBool,
        deadline: chrono::DateTime<Utc>,
    ) -> Result<bool, AppError> {
        loop {
            if cancellation.load(Ordering::Acquire) {
                self.finish_cancelled(job_id).await?;
                return Ok(true);
            }
            self.check_context()?;
            let Ok(remaining) = (deadline - Utc::now()).to_std() else {
                break;
            };
            if remaining.is_zero() {
                break;
            }
            tokio::time::sleep(remaining.min(Duration::from_millis(200))).await;
            self.check_context()?;
        }
        if cancellation.load(Ordering::Acquire) {
            self.finish_cancelled(job_id).await?;
            return Ok(true);
        }

        self.check_context()?;
        self.transition(|_, jobs| {
            let job = jobs.get_mut(&job_id).ok_or(AppError::NotFound)?;
            job.status = JobStatus::Running;
            job.clear_retry();
            job.updated_at = Utc::now();
            Ok(())
        })
        .await?;
        Ok(false)
    }
}

struct TelegramFrozenDriver<'a> {
    service: &'a TelegramCleanup,
    job_id: Uuid,
    cancellation: &'a AtomicBool,
}
#[async_trait::async_trait]
impl crate::providers::lifecycle::FrozenBatchDriver for TelegramFrozenDriver<'_> {
    type Target = i64;
    type Batch = cleaner_domain::DeletionBatch;
    type Error = AppError;
    type Retry = u64;
    fn targets<'a>(&self, batch: &'a Self::Batch) -> &'a [i64] {
        &batch.message_ids
    }
    async fn cancelled(&self) -> Result<bool, AppError> {
        self.service
            .stop_if_cancelled(self.job_id, self.cancellation)
            .await
    }
    async fn reach(&self, batch: &Self::Batch, id: &i64) -> Result<bool, AppError> {
        self.service
            .read
            .current_reach(batch.chat_id, *id)
            .await
            .map(|r| r == Some(DeletionReach::Everyone))
    }
    async fn mutate(&self, batch: &Self::Batch, ids: &[i64]) -> Result<(), AppError> {
        self.service
            .mutation
            .delete_messages_for_everyone(batch.chat_id, ids)
            .await
    }
    fn retry(&self, error: &AppError) -> Option<u64> {
        telegram_retry_after(error)
    }
    fn fatal(&self, error: &AppError) -> bool {
        matches!(error_code(error), "ambiguous_outcome" | "stale_context")
    }
    fn uncertain(&self, error: &AppError) -> bool {
        error_code(error) == "ambiguous_outcome"
    }
    async fn wait(&self, seconds: u64) -> Result<bool, AppError> {
        self.service
            .wait_for_retry(self.job_id, self.cancellation, seconds)
            .await
    }
    async fn progress(
        &self,
        progress: crate::providers::lifecycle::FrozenProgress<'_, AppError>,
    ) -> Result<(), AppError> {
        let crate::providers::lifecycle::FrozenProgress {
            next,
            skipped,
            deleted,
            failed,
            uncertain,
            error,
            fatal,
        } = progress;
        self.service
            .transition(|_, jobs| {
                let job = jobs.get_mut(&self.job_id).ok_or(AppError::NotFound)?;
                job.skipped += skipped;
                job.deleted += deleted;
                job.failed += failed;
                job.uncertain += uncertain;
                job.next_batch = next;
                job.clear_retry();
                if let Some(error) = error {
                    push_error_once(job, error_code(error));
                }
                if fatal {
                    job.status = if job.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                }
                job.updated_at = Utc::now();
                Ok(())
            })
            .await
    }
}
