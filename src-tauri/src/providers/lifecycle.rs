//! Shared lifecycle primitives extracted from the Telegram executor. Provider
//! callbacks interpret native IDs; this module never does. There is one frozen
//! batch loop, also called by Telegram's existing compound-job executor.
use crate::{error::AppError, persistence::FoundationStore, provider_service::safe};
use retract_domain::{ActiveContext, ErrorCode, RemediationPlan, Scope, ScopedJobRecord};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Default)]
pub struct GrantBook {
    grants: HashMap<Uuid, Grant>,
}
struct Grant {
    fingerprint: String,
    context: Option<ActiveContext>,
    expires_at: Instant,
}
impl GrantBook {
    pub fn issue(&mut self, id: Uuid, fingerprint: String, context: Option<ActiveContext>) {
        self.grants.insert(
            id,
            Grant {
                fingerprint,
                context,
                expires_at: Instant::now() + Duration::from_secs(60),
            },
        );
    }
    pub fn consume(
        &mut self,
        id: Uuid,
        fingerprint: &str,
        context: Option<&ActiveContext>,
    ) -> bool {
        self.grants.remove(&id).is_some_and(|grant| {
            grant.fingerprint == fingerprint
                && grant.context.as_ref() == context
                && grant.expires_at >= Instant::now()
        })
    }
    pub fn clear(&mut self) {
        self.grants.clear();
    }
}

/// The callbacks own native grouping, authority checks and effect-specific I/O.
/// Progress must be durable before returning. The helper rechecks cancellation
/// after preflight, retries the same frozen batch, and stops on uncertainty.
#[async_trait::async_trait]
pub trait FrozenBatchDriver: Sync {
    type Target: Clone + Send + Sync;
    type Batch: Send + Sync;
    type Error: Send + Sync;
    fn targets<'a>(&self, batch: &'a Self::Batch) -> &'a [Self::Target];
    async fn cancelled(&self) -> Result<bool, Self::Error>;
    async fn reach(&self, batch: &Self::Batch, target: &Self::Target) -> Result<bool, Self::Error>;
    async fn mutate(
        &self,
        batch: &Self::Batch,
        targets: &[Self::Target],
    ) -> Result<(), Self::Error>;
    fn retry_seconds(&self, error: &Self::Error) -> Option<u64>;
    fn fatal(&self, error: &Self::Error) -> bool;
    fn uncertain(&self, error: &Self::Error) -> bool;
    async fn wait(&self, seconds: u64) -> Result<bool, Self::Error>;
    async fn progress(&self, progress: FrozenProgress<'_, Self::Error>) -> Result<(), Self::Error>;
}

pub struct FrozenProgress<'a, E> {
    pub next: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub failed: usize,
    pub uncertain: usize,
    pub error: Option<&'a E>,
    pub fatal: bool,
}

pub async fn run_frozen_batches<D: FrozenBatchDriver>(
    driver: &D,
    batches: Vec<D::Batch>,
    next: usize,
) -> Result<bool, D::Error> {
    for (index, batch) in batches.into_iter().enumerate().skip(next) {
        loop {
            if driver.cancelled().await? {
                return Ok(true);
            }
            let mut allowed = Vec::new();
            let mut skipped = 0;
            let mut reach_error = None;
            for target in driver.targets(&batch) {
                match driver.reach(&batch, target).await {
                    Ok(true) => allowed.push(target.clone()),
                    Ok(false) => skipped += 1,
                    Err(error) => {
                        reach_error = Some(error);
                        break;
                    }
                }
            }
            if driver.cancelled().await? {
                return Ok(true);
            }
            let affected = if reach_error.is_some() {
                driver.targets(&batch).len().saturating_sub(skipped)
            } else {
                allowed.len()
            };
            let result = if let Some(error) = reach_error {
                Err(error)
            } else if allowed.is_empty() {
                Ok(())
            } else {
                driver.mutate(&batch, &allowed).await
            };
            if let Err(error) = &result
                && let Some(seconds) = driver.retry_seconds(error)
            {
                if driver.wait(seconds).await? {
                    return Ok(true);
                }
                continue;
            }
            let fatal = result.as_ref().err().is_some_and(|e| driver.fatal(e));
            let uncertain = result.as_ref().err().is_some_and(|e| driver.uncertain(e));
            driver
                .progress(FrozenProgress {
                    next: index + 1,
                    skipped,
                    deleted: if result.is_ok() { allowed.len() } else { 0 },
                    failed: if result.is_err() && !uncertain {
                        affected
                    } else {
                        0
                    },
                    uncertain: if uncertain { affected } else { 0 },
                    error: result.as_ref().err(),
                    fatal,
                })
                .await?;
            if fatal {
                return Ok(true);
            }
            break;
        }
    }
    Ok(false)
}

pub type Projection = (Vec<RemediationPlan>, Vec<ScopedJobRecord>);

/// Whole-scope CAS publication shared by compatibility and neutral lifecycles.
pub struct ScopedRepository {
    store: Arc<FoundationStore>,
    scope: Scope,
    last: std::sync::Mutex<Option<Projection>>,
}
impl ScopedRepository {
    pub fn new(store: Arc<FoundationStore>, scope: Scope) -> Result<Self, AppError> {
        if !store.snapshot()?.sources.iter().any(|s| s.scope() == scope) {
            return Err(AppError::InvalidRequest("stale_context".into()));
        }
        block_foreign_jobs(&store, Some(&scope))?;
        Ok(Self {
            store,
            scope,
            last: std::sync::Mutex::new(None),
        })
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn load(&self) -> Result<Projection, AppError> {
        let mut last = self.last.lock().map_err(|_| AppError::StateUnavailable)?;
        let projection = self.projection(&self.store.snapshot()?);
        if last.is_none() {
            *last = Some(projection.clone());
        }
        Ok(projection)
    }
    pub fn envelope(&self, id: Uuid) -> Result<RemediationPlan, AppError> {
        self.store
            .snapshot()?
            .plans
            .into_iter()
            .find(|p| p.scope == self.scope && p.id == id)
            .ok_or(AppError::NotFound)
    }
    pub fn commit(
        &self,
        mut plans: Vec<RemediationPlan>,
        mut jobs: Vec<ScopedJobRecord>,
    ) -> Result<(), AppError> {
        plans.sort_by_key(|p| p.id);
        jobs.sort_by_key(|j| j.id);
        let mut last = self.last.lock().map_err(|_| AppError::StateUnavailable)?;
        let expected = last.as_ref().ok_or(AppError::StateUnavailable)?;
        if expected.0.iter().any(|old| !plans.iter().any(|p| p == old))
            || expected
                .1
                .iter()
                .any(|old| !jobs.iter().any(|j| j.id == old.id))
        {
            return Err(AppError::StatePersistenceFailed);
        }
        for job in &jobs {
            if let Some(old) = expected.1.iter().find(|j| j.id == job.id)
                && (job.next_batch < old.next_batch
                    || job.counters.deleted < old.counters.deleted
                    || job.counters.skipped < old.counters.skipped
                    || job.counters.failed < old.counters.failed
                    || job.counters.uncertain < old.counters.uncertain
                    || job.updated_at < old.updated_at
                    || (old.status.is_terminal() && !job.status.is_terminal()))
            {
                return Err(AppError::StatePersistenceFailed);
            }
        }
        self.store.transaction(|state| {
            if self.projection(state) != *expected {
                return Err(AppError::StatePersistenceFailed);
            }
            state.plans.retain(|p| p.scope != self.scope);
            state.jobs.retain(|j| j.scope != self.scope);
            state.plans.extend(plans.clone());
            state.jobs.extend(jobs.clone());
            Ok(())
        })?;
        *last = Some((plans, jobs));
        Ok(())
    }
    fn projection(&self, state: &crate::persistence::FoundationState) -> Projection {
        let mut plans = state
            .plans
            .iter()
            .filter(|p| p.scope == self.scope)
            .cloned()
            .collect::<Vec<_>>();
        let mut jobs = state
            .jobs
            .iter()
            .filter(|p| p.scope == self.scope)
            .cloned()
            .collect::<Vec<_>>();
        plans.sort_by_key(|p| p.id);
        jobs.sort_by_key(|p| p.id);
        (plans, jobs)
    }
}

pub fn resumable(plan: &RemediationPlan, job: &ScopedJobRecord) -> bool {
    job.started_authorized
        && plan.restart_policy == retract_domain::RestartPolicy::ResumeFrozenTargets
        && job.counters.uncertain == 0
        && !job.diagnostics.iter().any(|d| {
            matches!(
                d.code,
                ErrorCode::AmbiguousOutcome
                    | ErrorCode::StatePersistenceFailed
                    | ErrorCode::RestartRequiresNewReview
            )
        })
}
pub fn block_foreign_jobs(store: &FoundationStore, scope: Option<&Scope>) -> Result<(), AppError> {
    if !store.snapshot()?.jobs.iter().any(|j| {
        scope != Some(&j.scope)
            && !j.status.is_terminal()
            && j.status != retract_domain::JobStatus::Blocked
    }) {
        return Ok(());
    }
    store.transaction(|state| {
        for job in &mut state.jobs {
            if scope != Some(&job.scope) && !job.status.is_terminal() {
                if job.status == retract_domain::JobStatus::Running {
                    job.status = if job.counters.deleted > 0 {
                        retract_domain::JobStatus::Partial
                    } else {
                        retract_domain::JobStatus::Failed
                    };
                    job.diagnostics.push(safe(ErrorCode::AmbiguousOutcome));
                } else {
                    job.status = retract_domain::JobStatus::Blocked;
                    job.diagnostics.push(safe(if scope.is_some() {
                        ErrorCode::ScopeMismatch
                    } else {
                        ErrorCode::IdentityUnavailable
                    }));
                }
                job.retry_at = None;
            }
        }
        Ok(())
    })
}
