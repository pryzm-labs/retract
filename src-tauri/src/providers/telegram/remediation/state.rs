use super::*;
#[cfg(test)]
use crate::{
    providers::telegram::engine_context::LegacyTelegramRepository, secure_store::SecureJobStore,
};

impl TelegramCleanup {
    #[cfg(test)]
    pub fn new<G: TelegramRead + TelegramMutation + 'static>(
        gateway: Arc<G>,
        store: SecureJobStore,
    ) -> Result<Arc<Self>, AppError> {
        let persisted = store.load()?;
        let legacy_unbound = store.loaded_legacy_unbound();
        let plans: HashMap<Uuid, DeletionPlan> = persisted
            .plans
            .into_iter()
            .map(|plan| (plan.id, plan))
            .collect();
        let jobs = persisted
            .jobs
            .into_iter()
            .map(|mut job| {
                if let Some(plan) = plans.get(&job.plan_id) {
                    job.backfill_target_chat_ids(plan);
                }
                if legacy_unbound && matches!(job.status, JobStatus::Queued | JobStatus::Running) {
                    job.status = if job.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                    job.clear_retry();
                    push_error_once(&mut job, "legacy_store_requires_new_review");
                } else if matches!(job.status, JobStatus::Queued | JobStatus::Running)
                    && matches!(
                        job.operation,
                        PlanOperation::ClearHistory
                            | PlanOperation::ClearHistoryAndLeave
                            | PlanOperation::RemoveChatForSelf
                            | PlanOperation::DeleteBySender
                            | PlanOperation::DeleteGroup
                    )
                {
                    job.status = if job.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                    job.clear_retry();
                    push_error_once(&mut job, "restart_requires_new_review");
                } else if matches!(job.status, JobStatus::Running) {
                    job.status = JobStatus::Queued;
                    push_error_once(&mut job, "resumed_after_restart");
                }
                (job.id, job)
            })
            .collect();
        Ok(Arc::new_cyclic(|worker_owner| Self {
            read: gateway.clone(),
            mutation: gateway,
            worker_owner: worker_owner.clone(),
            plans: RwLock::new(plans),
            jobs: RwLock::new(jobs),
            cancellation: Mutex::new(HashMap::new()),
            system_grants: Mutex::new(crate::providers::lifecycle::GrantBook::default()),
            store: Arc::new(LegacyTelegramRepository(store)),
            context: None,
            transition_lock: Mutex::new(()),
            persistence_failed: AtomicBool::new(false),
        }))
    }

    pub fn new_scoped(
        read: Arc<dyn TelegramRead>,
        mutation: Arc<dyn TelegramMutation>,
        context: Arc<EngineContext>,
        store: Arc<dyn TelegramStateRepository>,
    ) -> Result<Arc<Self>, AppError> {
        context.check(read.as_ref())?;
        context.check(mutation.as_ref())?;
        if store.scope() != Some(&context.active().scope) {
            return Err(crate::providers::telegram::engine_context::stale_context());
        }
        let persisted = store.load()?;
        // Recovery decisions are durable before any state or worker is published.
        store.save(&persisted)?;
        Ok(Arc::new_cyclic(|worker_owner| Self {
            read: Arc::new(SessionRead {
                inner: read,
                context: context.clone(),
            }),
            mutation: Arc::new(SessionMutation {
                inner: mutation,
                context: context.clone(),
            }),
            worker_owner: worker_owner.clone(),
            plans: RwLock::new(persisted.plans.into_iter().map(|p| (p.id, p)).collect()),
            jobs: RwLock::new(persisted.jobs.into_iter().map(|j| (j.id, j)).collect()),
            cancellation: Mutex::new(HashMap::new()),
            system_grants: Mutex::new(crate::providers::lifecycle::GrantBook::default()),
            store,
            context: Some(context),
            transition_lock: Mutex::new(()),
            persistence_failed: AtomicBool::new(false),
        }))
    }

    pub(super) fn check_context(&self) -> Result<(), AppError> {
        if self.persistence_failed.load(Ordering::Acquire) {
            return Err(AppError::StatePersistenceFailed);
        }
        if let Some(context) = &self.context {
            context.check(self.read.as_ref())?;
            context.check(self.mutation.as_ref())?;
        }
        Ok(())
    }

    pub(crate) fn reviewed_plan(
        &self,
        id: Uuid,
    ) -> Result<retract_domain::RemediationPlan, AppError> {
        self.store.envelope(id)
    }
    pub(crate) async fn has_workers(&self) -> bool {
        !self.cancellation.lock().await.is_empty()
    }
    pub(crate) async fn stop_workers(&self) {
        for token in self.cancellation.lock().await.values() {
            token.store(true, Ordering::Release);
        }
        loop {
            if self.cancellation.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.system_grants.lock().await.clear();
    }

    pub(super) async fn publish_plan(&self, mut plan: DeletionPlan) -> Result<PlanView, AppError> {
        self.check_context()?;
        if let Some(context) = &self.context {
            context.bind_plan(&mut plan)?;
        }
        let view = PlanView::from(&plan);
        self.transition(|plans, _| {
            plans.insert(plan.id, plan);
            Ok(())
        })
        .await?;
        Ok(view)
    }

    /// R3: serialize snapshot, mutation, durable save and publication together.
    /// Failed candidates never become executable or advance recoverable state.
    pub(super) async fn transition<T>(
        &self,
        change: impl FnOnce(
            &mut HashMap<Uuid, DeletionPlan>,
            &mut HashMap<Uuid, JobRecord>,
        ) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let _serial = self.transition_lock.lock().await;
        if self.persistence_failed.load(Ordering::Acquire) {
            return Err(AppError::StatePersistenceFailed);
        }
        let mut plans = self.plans.read().await.clone();
        let mut jobs = self.jobs.read().await.clone();
        let result = change(&mut plans, &mut jobs)?;
        let state = PersistedState {
            plans: plans.values().cloned().collect(),
            jobs: jobs.values().cloned().collect(),
        };
        if self.store.save(&state).is_err() {
            self.persistence_failed.store(true, Ordering::Release);
            if let Some(context) = &self.context {
                context.quarantine();
            }
            return Err(AppError::StatePersistenceFailed);
        }
        *self.plans.write().await = plans;
        *self.jobs.write().await = jobs;
        Ok(result)
    }

    pub(crate) async fn cancel_job(&self, job_id: Uuid) -> Result<JobRecord, AppError> {
        let jobs = self.jobs.read().await;
        let job = jobs.get(&job_id).cloned().ok_or(AppError::NotFound)?;
        if job.status.is_terminal() {
            return Err(AppError::JobAlreadyTerminal);
        }
        drop(jobs);
        let cancellation = self
            .cancellation
            .lock()
            .await
            .get(&job_id)
            .cloned()
            .ok_or(AppError::StateUnavailable)?;
        cancellation.store(true, Ordering::Release);
        Ok(job)
    }

    pub(crate) async fn legacy_jobs(&self) -> Vec<JobRecord> {
        let mut jobs: Vec<_> = self.jobs.read().await.values().cloned().collect();
        if self.persistence_failed.load(Ordering::Acquire) {
            for job in &mut jobs {
                if !job.status.is_terminal() {
                    job.status = if job.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                    job.clear_retry();
                    push_error_once(job, "state_persistence_failed");
                }
            }
        }
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs.truncate(50);
        jobs
    }

    pub(super) async fn persist(&self) -> Result<(), AppError> {
        self.transition(|_, _| Ok(())).await
    }
}

pub(super) fn push_error_once(job: &mut JobRecord, code: &str) {
    if !job.error_codes.iter().any(|existing| existing == code) {
        job.error_codes.push(code.into());
    }
}
