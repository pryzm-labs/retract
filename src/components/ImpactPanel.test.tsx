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
  it("renders every current job state with truthful progress and diagnostics", () => {
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

    expect(screen.getByText("queued")).toBeInTheDocument();
    expect(screen.getByText("running · 4 deleted")).toBeInTheDocument();
    expect(
      screen.getByText("rate limited · retry in 3s · 4 deleted"),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Cancel" })).toHaveLength(3);

    const queuedRow = screen.getByText("queued").closest(".job-row");
    const runningRow = screen
      .getByText("running · 4 deleted")
      .closest(".job-row");
    const retryRow = screen
      .getByText("rate limited · retry in 3s · 4 deleted")
      .closest(".job-row");
    fireEvent.click(within(queuedRow as HTMLElement).getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(runningRow as HTMLElement).getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(retryRow as HTMLElement).getByRole("button", { name: "Cancel" }));
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

    expect(screen.getByText("completed · 2 deleted")).toBeInTheDocument();
    expect(screen.getByText("partial · 3 deleted")).toBeInTheDocument();
    expect(screen.getByText("failed")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();

    const cancelled = job("cancelled-job", "cancelled", {
      total: 4,
      deleted: 1,
      skipped: 3,
      nextBatch: 1,
      errorCodes: ["synthetic_cancelled"],
    });
    rerender(<ImpactPanel {...panelProps([cancelled], onCancelJob)} />);

    expect(screen.getByText("cancelled · 1 deleted")).toBeInTheDocument();
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
