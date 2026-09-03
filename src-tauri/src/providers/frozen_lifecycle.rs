//! A neutral binding of the extracted frozen-target lifecycle. Providers supply
//! typed recipes/descriptors and native I/O only; grants, jobs, persistence,
//! recovery, cancellation and the batch runner are production shared code.
use super::{
    lifecycle::{
        FrozenBatchDriver, GrantBook, Projection, ScopedRepository, resumable, run_frozen_batches,
    },
    ports::*,
};
use crate::{error::boundary_error, persistence::FoundationStore, provider_service::safe};
use async_trait::async_trait;
use chrono::Utc;
use retract_domain::*;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[async_trait]
pub trait FrozenProviderIo: Send + Sync {
    fn check(&self, context: &ActiveContext) -> Result<(), SafeError>;
    async fn describe(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
        id: Uuid,
    ) -> Result<RemediationPlan, SafeError>;
    fn dirty_refs(&self, plan: &RemediationPlan) -> Result<Vec<ScopedResourceRef>, SafeError>;
    async fn owner_prompt(&self, plan: &RemediationPlan) -> Result<(), SafeError>;
    async fn preflight(&self, target: &ScopedResourceRef) -> Result<bool, SafeError>;
    async fn mutate(
        &self,
        plan: &RemediationPlan,
        targets: &[ScopedResourceRef],
    ) -> Result<(), SafeError>;
    async fn intents(
        &self,
        context: &ActiveContext,
        targets: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, SafeError>;
}

#[derive(Clone)]
pub struct FrozenLifecycle(Arc<Inner>);
struct Inner {
    context: ActiveContext,
    io: Arc<dyn FrozenProviderIo>,
    repository: ScopedRepository,
    state: Mutex<Projection>,
    grants: Mutex<GrantBook>,
    workers: Mutex<HashMap<Uuid, Arc<AtomicBool>>>,
    failed: AtomicBool,
    stopped: AtomicBool,
}
impl FrozenLifecycle {
    pub fn new(
        context: ActiveContext,
        io: Arc<dyn FrozenProviderIo>,
        store: Arc<FoundationStore>,
    ) -> Result<Self, SafeError> {
        io.check(&context)?;
        let repository =
            ScopedRepository::new(store, context.scope.clone()).map_err(boundary_error)?;
        let mut state = repository.load().map_err(boundary_error)?;
        for job in &mut state.1 {
            if job.status.is_terminal() {
                continue;
            }
            let plan = state
                .0
                .iter()
                .find(|p| p.id == job.plan_id)
                .ok_or_else(|| safe(ErrorCode::NotFound))?;
            if resumable(plan, job) {
                job.status = JobStatus::Queued;
            } else {
                job.status = if job.counters.deleted > 0 && job.started_authorized {
                    JobStatus::Partial
                } else {
                    JobStatus::Failed
                };
                job.retry_at = None;
                job.diagnostics
                    .push(safe(ErrorCode::RestartRequiresNewReview));
            }
        }
        repository
            .commit(state.0.clone(), state.1.clone())
            .map_err(boundary_error)?;
        Ok(Self(Arc::new(Inner {
            context,
            io,
            repository,
            state: Mutex::new(state),
            grants: Mutex::new(GrantBook::default()),
            workers: Mutex::new(HashMap::new()),
            failed: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        })))
    }
    fn check(&self, context: &ActiveContext) -> Result<(), SafeError> {
        if self.0.failed.load(Ordering::Acquire) {
            return Err(safe(ErrorCode::StatePersistenceFailed));
        }
        if self.0.stopped.load(Ordering::Acquire) || context != &self.0.context {
            return Err(safe(ErrorCode::StaleContext));
        }
        self.0.io.check(context)
    }
    async fn transition<T>(
        &self,
        change: impl FnOnce(&mut Projection) -> Result<T, SafeError>,
    ) -> Result<T, SafeError> {
        let mut state = self.0.state.lock().await;
        if self.0.failed.load(Ordering::Acquire) {
            return Err(safe(ErrorCode::StatePersistenceFailed));
        }
        let mut candidate = state.clone();
        let result = change(&mut candidate)?;
        if self
            .0
            .repository
            .commit(candidate.0.clone(), candidate.1.clone())
            .is_err()
        {
            self.0.failed.store(true, Ordering::Release);
            return Err(safe(ErrorCode::StatePersistenceFailed));
        }
        *state = candidate;
        Ok(result)
    }
    async fn plan(&self, request: &ReviewedPlanRef) -> Result<RemediationPlan, SafeError> {
        let plan = self
            .0
            .repository
            .envelope(request.plan_id)
            .map_err(boundary_error)?;
        if plan.fingerprint != request.fingerprint {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        Ok(plan)
    }
    async fn schedule(&self, id: Uuid) {
        let mut workers = self.0.workers.lock().await;
        if workers.contains_key(&id) {
            return;
        }
        workers.insert(id, Arc::new(AtomicBool::new(false)));
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = this.run(id).await {
                let _ = this
                    .transition(|state| {
                        let job = state
                            .1
                            .iter_mut()
                            .find(|j| j.id == id)
                            .ok_or_else(|| safe(ErrorCode::NotFound))?;
                        if !job.status.is_terminal() {
                            job.status = if job.counters.deleted > 0 {
                                JobStatus::Partial
                            } else {
                                JobStatus::Failed
                            };
                            job.retry_at = None;
                            job.diagnostics.push(error);
                            job.updated_at = Utc::now();
                        }
                        Ok(())
                    })
                    .await;
            }
            this.0.workers.lock().await.remove(&id);
        });
    }
    async fn run(&self, id: Uuid) -> Result<(), SafeError> {
        self.check(&self.0.context)?;
        let job = self
            .0
            .state
            .lock()
            .await
            .1
            .iter()
            .find(|j| j.id == id)
            .cloned()
            .ok_or_else(|| safe(ErrorCode::NotFound))?;
        let plan = self
            .0
            .repository
            .envelope(job.plan_id)
            .map_err(boundary_error)?;
        let driver = Driver {
            lifecycle: self,
            id,
            plan: &plan,
        };
        if let Some(deadline) = job.retry_at
            && driver.wait_until(deadline).await?
        {
            return Ok(());
        }
        self.transition(|s| {
            let j = s.1.iter_mut().find(|j| j.id == id).unwrap();
            j.status = JobStatus::Running;
            j.updated_at = Utc::now();
            Ok(())
        })
        .await?;
        let mut batches = Vec::new();
        for step in &plan.steps {
            if step.descriptor.kind != ActionKind::DeleteRemoteItem {
                return Err(safe(ErrorCode::UnsupportedSchema));
            }
            batches.extend(
                step.targets
                    .chunks(step.descriptor.batch.max_targets as usize)
                    .map(|chunk| chunk.to_vec()),
            );
        }
        if run_frozen_batches(&driver, batches, job.next_batch as usize).await? {
            return Ok(());
        }
        self.transition(|s| {
            let j = s.1.iter_mut().find(|j| j.id == id).unwrap();
            j.status = if j.counters.failed > 0 {
                JobStatus::Partial
            } else {
                JobStatus::Completed
            };
            j.retry_at = None;
            j.updated_at = Utc::now();
            Ok(())
        })
        .await
    }
}

#[async_trait]
impl ReviewedLifecycle for FrozenLifecycle {
    async fn intents(
        &self,
        context: &ActiveContext,
        targets: Vec<ScopedResourceRef>,
    ) -> Result<Vec<IntentDescriptor>, SafeError> {
        self.check(context)?;
        let result = self.0.io.intents(context, targets).await?;
        self.check(context)?;
        Ok(result)
    }
    async fn prepare(
        &self,
        context: &ActiveContext,
        intent: PrepareIntent,
    ) -> Result<RemediationPlan, SafeError> {
        self.check(context)?;
        let id = Uuid::new_v4();
        let mut plan = self.0.io.describe(context, intent, id).await?;
        self.check(context)?;
        if plan.id != id || plan.scope != context.scope {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        plan.seal().map_err(|_| safe(ErrorCode::ScopeMismatch))?;
        self.transition(|s| {
            s.0.push(plan.clone());
            Ok(())
        })
        .await?;
        Ok(plan)
    }
    async fn authorize(
        &self,
        context: &ActiveContext,
        request: ReviewedPlanRef,
    ) -> Result<(), SafeError> {
        self.check(context)?;
        let plan = self.plan(&request).await?;
        self.0.io.owner_prompt(&plan).await?;
        let mut grants = self.0.grants.lock().await;
        self.check(context)?;
        grants.issue(plan.id, plan.fingerprint, Some(context.clone()));
        Ok(())
    }
    async fn start(
        &self,
        context: &ActiveContext,
        request: StartReviewed,
    ) -> Result<ScopedJobRecord, SafeError> {
        self.check(context)?;
        let plan = self
            .plan(&ReviewedPlanRef {
                plan_id: request.plan_id,
                fingerprint: request.fingerprint,
            })
            .await?;
        if !request.irreversible_acknowledged
            || plan.confirmation.exact_text.is_some()
                && plan.confirmation.exact_text != request.typed_chat_title
        {
            return Err(safe(ErrorCode::ScopeMismatch));
        }
        let mut grants = self.0.grants.lock().await;
        self.check(context)?;
        if !grants.consume(plan.id, &plan.fingerprint, Some(context)) {
            return Err(safe(ErrorCode::AuthenticationRequired));
        }
        drop(grants);
        let job = ScopedJobRecord {
            id: Uuid::new_v4(),
            plan_id: plan.id,
            scope: context.scope.clone(),
            dirty_refs: self.0.io.dirty_refs(&plan)?,
            status: JobStatus::Queued,
            counters: JobCounters {
                selected: plan.targets.len() as u64,
                eligible: plan.targets.len() as u64,
                ..JobCounters::default()
            },
            next_batch: 0,
            retry_at: None,
            diagnostics: vec![],
            started_authorized: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.transition(|s| {
            self.check(context)?;
            if s.1.iter().any(|j| j.plan_id == plan.id) {
                return Err(safe(ErrorCode::PermissionChanged));
            }
            s.1.push(job.clone());
            Ok(())
        })
        .await?;
        self.schedule(job.id).await;
        Ok(job)
    }
    async fn jobs(&self, context: &ActiveContext) -> Result<Vec<ScopedJobRecord>, SafeError> {
        self.check(context)?;
        Ok(self.0.state.lock().await.1.clone())
    }
    async fn cancel(
        &self,
        context: &ActiveContext,
        id: Uuid,
    ) -> Result<ScopedJobRecord, SafeError> {
        self.check(context)?;
        let workers = self.0.workers.lock().await;
        workers
            .get(&id)
            .ok_or_else(|| safe(ErrorCode::NotFound))?
            .store(true, Ordering::Release);
        self.0
            .state
            .lock()
            .await
            .1
            .iter()
            .find(|j| j.id == id)
            .cloned()
            .ok_or_else(|| safe(ErrorCode::NotFound))
    }
    async fn recover(&self, context: &ActiveContext) -> Result<(), SafeError> {
        self.check(context)?;
        let jobs = self.0.state.lock().await.1.clone();
        for job in jobs {
            if !job.status.is_terminal() {
                self.schedule(job.id).await;
            }
        }
        Ok(())
    }
    async fn has_workers(&self) -> bool {
        !self.0.workers.lock().await.is_empty()
    }
    async fn stop(&self) {
        self.0.stopped.store(true, Ordering::Release);
        for token in self.0.workers.lock().await.values() {
            token.store(true, Ordering::Release);
        }
        while !self.0.workers.lock().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        self.0.grants.lock().await.clear();
    }
}

struct Driver<'a> {
    lifecycle: &'a FrozenLifecycle,
    id: Uuid,
    plan: &'a RemediationPlan,
}
impl Driver<'_> {
    async fn wait_until(&self, deadline: chrono::DateTime<Utc>) -> Result<bool, SafeError> {
        loop {
            if self.cancelled().await? {
                return Ok(true);
            }
            self.lifecycle.check(&self.lifecycle.0.context)?;
            let Ok(remaining) = (deadline - Utc::now()).to_std() else {
                break;
            };
            if remaining.is_zero() {
                break;
            }
            tokio::time::sleep(remaining.min(std::time::Duration::from_millis(200))).await;
            self.lifecycle.check(&self.lifecycle.0.context)?;
        }
        if self.cancelled().await? {
            return Ok(true);
        }
        self.lifecycle
            .transition(|s| {
                let j = s.1.iter_mut().find(|j| j.id == self.id).unwrap();
                j.retry_at = None;
                j.status = JobStatus::Running;
                j.updated_at = Utc::now();
                Ok(())
            })
            .await?;
        Ok(false)
    }
}
#[async_trait]
impl FrozenBatchDriver for Driver<'_> {
    type Target = ScopedResourceRef;
    type Batch = Vec<ScopedResourceRef>;
    type Error = SafeError;
    fn targets<'a>(&self, batch: &'a Self::Batch) -> &'a [Self::Target] {
        batch
    }
    async fn cancelled(&self) -> Result<bool, SafeError> {
        let cancelled = self
            .lifecycle
            .0
            .workers
            .lock()
            .await
            .get(&self.id)
            .is_none_or(|token| token.load(Ordering::Acquire));
        if cancelled {
            self.lifecycle
                .transition(|s| {
                    let j = s.1.iter_mut().find(|j| j.id == self.id).unwrap();
                    j.status = JobStatus::Cancelled;
                    j.retry_at = None;
                    j.updated_at = Utc::now();
                    Ok(())
                })
                .await?;
        }
        Ok(cancelled)
    }
    async fn reach(&self, _: &Self::Batch, target: &Self::Target) -> Result<bool, SafeError> {
        self.lifecycle.check(&self.lifecycle.0.context)?;
        let result = self.lifecycle.0.io.preflight(target).await?;
        self.lifecycle.check(&self.lifecycle.0.context)?;
        Ok(result)
    }
    async fn mutate(&self, _: &Self::Batch, targets: &[Self::Target]) -> Result<(), SafeError> {
        self.lifecycle.check(&self.lifecycle.0.context)?;
        let result = self.lifecycle.0.io.mutate(self.plan, targets).await;
        if self.lifecycle.check(&self.lifecycle.0.context).is_err() {
            return Err(safe(ErrorCode::AmbiguousOutcome));
        }
        result
    }
    fn retry_seconds(&self, error: &SafeError) -> Option<u64> {
        if error.code == ErrorCode::RateLimited {
            error
                .retry_at
                .map(|d| (d - Utc::now()).num_seconds().max(1) as u64)
        } else {
            None
        }
    }
    fn fatal(&self, error: &SafeError) -> bool {
        matches!(
            error.code,
            ErrorCode::AmbiguousOutcome
                | ErrorCode::StaleContext
                | ErrorCode::ScopeMismatch
                | ErrorCode::IdentityUnavailable
        )
    }
    fn uncertain(&self, error: &SafeError) -> bool {
        error.code == ErrorCode::AmbiguousOutcome
    }
    async fn wait(&self, seconds: u64) -> Result<bool, SafeError> {
        let deadline = Utc::now() + chrono::Duration::seconds(seconds.min(86400) as i64);
        self.lifecycle
            .transition(|s| {
                let j = s.1.iter_mut().find(|j| j.id == self.id).unwrap();
                j.status = JobStatus::Queued;
                j.retry_at = Some(deadline);
                j.diagnostics.push(SafeError {
                    code: ErrorCode::RateLimited,
                    retry_at: Some(deadline),
                });
                j.updated_at = Utc::now();
                Ok(())
            })
            .await?;
        self.wait_until(deadline).await
    }
    async fn progress(
        &self,
        progress: super::lifecycle::FrozenProgress<'_, SafeError>,
    ) -> Result<(), SafeError> {
        let super::lifecycle::FrozenProgress {
            next,
            skipped,
            deleted,
            failed,
            uncertain,
            error,
            fatal,
        } = progress;
        self.lifecycle
            .transition(|s| {
                let j = s.1.iter_mut().find(|j| j.id == self.id).unwrap();
                j.next_batch = next as u64;
                j.counters.skipped += skipped as u64;
                j.counters.deleted += deleted as u64;
                j.counters.failed += failed as u64;
                j.counters.uncertain += uncertain as u64;
                j.retry_at = None;
                if let Some(error) = error {
                    j.diagnostics.push(error.clone());
                }
                if fatal {
                    j.status = if j.counters.deleted > 0 {
                        JobStatus::Partial
                    } else {
                        JobStatus::Failed
                    };
                }
                j.updated_at = Utc::now();
                Ok(())
            })
            .await
    }
}
