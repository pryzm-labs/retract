import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { JobRecord, JobStatus } from "../types";
import { ImpactPanel } from "./ImpactPanel";

const timestamp = "2026-01-02T03:04:05.000Z";

function job(
  id: string,
  status: JobStatus,
  overrides: Partial<JobRecord> = {},
): JobRecord {
  return {
    id,
    planId: `plan-${id}`,
    operation: "selected_messages",
    targetChatIds: [-2101],
    status,
    total: 9,
    deleted: 0,
    skipped: 0,
    failed: 0,
    nextBatch: 0,
    retryAfterSeconds: null,
    errorCodes: [],
    createdAt: timestamp,
    updatedAt: timestamp,
    ...overrides,
  };
}

function panelProps(
  jobs: JobRecord[],
  onCancelJob = vi.fn(),
): ComponentProps<typeof ImpactPanel> {
  return {
    selected: [],
    jobs,
    busy: false,
    busyLabel: null,
    chatRemovalPending: false,
    hiddenSelectionCount: 0,
    onReview: vi.fn(),
    onChatAction: vi.fn(),
    onOwnMessagesAction: vi.fn(),
    onSenderAction: vi.fn(),
    onClearSelection: vi.fn(),
    onCancelJob,
  };
}

describe("ImpactPanel job activity", () => {
  it("renders every current job state and only the counters production exposes", () => {
    const onCancelJob = vi.fn();
    const queued = job("queued-job", "queued");
    const running = job("running-job", "running", {
      deleted: 4,
      skipped: 1,
      nextBatch: 1,
    });
    const rateLimited = job("rate-limited-job", "running", {
      deleted: 4,
      retryAfterSeconds: 3,
      errorCodes: ["synthetic_rate_limited"],
    });
    const { rerender } = render(
      <ImpactPanel {...panelProps([queued, running, rateLimited], onCancelJob)} />,
    );

    const queuedRow = screen.getByRole("group", { name: "Cleanup job queued-job" });
    const runningRow = screen.getByRole("group", { name: "Cleanup job running-job" });
    const retryRow = screen.getByRole("group", { name: "Cleanup job rate-limited-job" });
    expect(within(queuedRow).getByText("queued")).toBeInTheDocument();
    expect(within(runningRow).getByText("running · 4 deleted")).toBeInTheDocument();
    expect(
      within(retryRow).getByText("rate limited · retry in 3s · 4 deleted"),
    ).toBeInTheDocument();
    expect(within(runningRow).queryByText(/1 skipped/i)).not.toBeInTheDocument();
    expect(within(retryRow).queryByText(/synthetic_rate_limited/i)).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Cancel" })).toHaveLength(3);

    fireEvent.click(within(queuedRow).getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(runningRow).getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(retryRow).getByRole("button", { name: "Cancel" }));
    expect(onCancelJob.mock.calls).toEqual([
      ["queued-job"],
      ["running-job"],
      ["rate-limited-job"],
    ]);

    const completed = job("completed-job", "completed", {
      total: 2,
      deleted: 2,
      nextBatch: 1,
    });
    const partial = job("partial-job", "partial", {
      total: 5,
      deleted: 3,
      skipped: 1,
      failed: 1,
      nextBatch: 1,
      errorCodes: ["synthetic_partial"],
    });
    const failed = job("failed-job", "failed", {
      total: 3,
      failed: 3,
      nextBatch: 1,
      errorCodes: ["synthetic_failure"],
    });
    rerender(
      <ImpactPanel {...panelProps([completed, partial, failed], onCancelJob)} />,
    );

    const completedRow = screen.getByRole("group", { name: "Cleanup job completed-job" });
    const partialRow = screen.getByRole("group", { name: "Cleanup job partial-job" });
    const failedRow = screen.getByRole("group", { name: "Cleanup job failed-job" });
    expect(within(completedRow).getByText("completed · 2 deleted")).toBeInTheDocument();
    expect(within(partialRow).getByText("partial · 3 deleted")).toBeInTheDocument();
    expect(within(failedRow).getByText("failed")).toBeInTheDocument();
    expect(within(partialRow).queryByText(/1 skipped/i)).not.toBeInTheDocument();
    expect(within(partialRow).queryByText(/1 failed/i)).not.toBeInTheDocument();
    expect(within(partialRow).queryByText(/synthetic_partial/i)).not.toBeInTheDocument();
    expect(within(failedRow).queryByText(/3 failed/i)).not.toBeInTheDocument();
    expect(within(failedRow).queryByText(/synthetic_failure/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();

    const cancelled = job("cancelled-job", "cancelled", {
      total: 4,
      deleted: 1,
      skipped: 3,
      nextBatch: 1,
      errorCodes: ["synthetic_cancelled"],
    });
    rerender(<ImpactPanel {...panelProps([cancelled], onCancelJob)} />);

    const cancelledRow = screen.getByRole("group", { name: "Cleanup job cancelled-job" });
    expect(within(cancelledRow).getByText("cancelled · 1 deleted")).toBeInTheDocument();
    expect(within(cancelledRow).queryByText(/3 skipped/i)).not.toBeInTheDocument();
    expect(within(cancelledRow).queryByText(/synthetic_cancelled/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
    expect(
      screen.getByText("Job logs contain IDs and counts, not message content."),
    ).toBeInTheDocument();
  });

  it("keeps cancellation visible but disabled while another action is busy", () => {
    render(
      <ImpactPanel
        {...panelProps([job("queued-job", "queued")])}
        busy
      />,
    );

    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  });
});
