import { fixtureContext, fixtureRef } from "../demo";
import { uuid } from "../providers/identity";
import { jobView } from "../providers/telegram";
import type { JobRecord, JobStatus } from "../types";
export const testId = (name: string) => uuid(fixtureRef("grouping", name).id);
export function testJob(id: string, status: JobStatus, overrides: Partial<JobRecord> = {}): JobRecord {
  const selected = overrides.total ?? 9;
  const timestamp = "2026-01-02T03:04:05.000Z";
  return { ...jobView({ id: testId(id), planId: testId("plan-" + id), scope: fixtureContext.scope, dirtyRefs: [fixtureRef("conversation", "-2101")], status,
    counters: { selected: selected + (overrides.skipped ?? 0), eligible: selected, deleted: overrides.deleted ?? 0, skipped: overrides.skipped ?? 0, failed: overrides.failed ?? 0, uncertain: 0 }, nextBatch: 0,
    retryAt: null, diagnostics: [], startedAuthorized: true, createdAt: timestamp, updatedAt: timestamp }), ...overrides };
}
