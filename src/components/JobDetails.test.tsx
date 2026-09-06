import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ImpactPanel } from "./ImpactPanel";
import { testJob } from "../test/v2-fixtures";
import { safeMessages, type SafeError } from "../providers/contract";
import type { JobRecord } from "../types";

function renderJobs(jobs: JobRecord[]) {
  return render(<ImpactPanel selected={[]} jobs={jobs} busy={false} busyLabel={null} chatRemovalPending={false} hiddenSelectionCount={0}
    onReview={vi.fn()} onChatAction={vi.fn()} onOwnMessagesAction={vi.fn()} onSenderAction={vi.fn()} onClearSelection={vi.fn()} onCancelJob={vi.fn()} />);
}
afterEach(() => vi.useRealTimers());

describe("accessible cleanup details", () => {
  it("discloses separate outcome counters and returns keyboard focus without expanding compact rows", () => {
    const job = testJob("outcomes", "partial", { total: 10, deleted: 2, skipped: 3, failed: 4,
      counters: { selected: 13, eligible: 10, deleted: 2, skipped: 3, failed: 4, uncertain: 1 },
      diagnostics: [{ code: "ambiguous_outcome", message: "RAW_SECRET", retryAt: null }] });
    renderJobs([job]);
    expect(screen.queryByText("Skipped")).not.toBeInTheDocument();
    const trigger = screen.getByRole("button", { name: `View details for cleanup job ${job.id}` });
    trigger.focus(); fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Cleanup job details" });
    for (const [label, value] of [["Selected", 13], ["Eligible", 10], ["Deleted", 2], ["Skipped", 3], ["Failed", 4], ["Uncertain", 1]]) {
      expect(within(dialog).getByRole("group", { name: label as string })).toHaveTextContent(String(value));
    }
    expect(within(dialog).getByText(safeMessages.ambiguous_outcome)).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("RAW_SECRET");
    expect(within(dialog).getByRole("button", { name: "Close job details" })).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("updates the retry countdown from the backend timestamp", () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date("2026-09-03T00:00:00Z"));
    const job = testJob("retry", "running", { retryAt: "2026-09-03T00:00:03Z" });
    renderJobs([job]); fireEvent.click(screen.getByRole("button", { name: `View details for cleanup job ${job.id}` }));
    expect(screen.getByRole("status", { name: "Retry state" })).toHaveTextContent("Retry in 3 seconds");
    act(() => vi.advanceTimersByTime(1000));
    expect(screen.getByRole("status", { name: "Retry state" })).toHaveTextContent("Retry in 2 seconds");
    act(() => vi.advanceTimersByTime(2000));
    expect(screen.getByRole("status", { name: "Retry state" })).toHaveTextContent("Waiting for provider update");
  });

  it("shows original-account and new-review guidance with safe fallback diagnostics", () => {
    const job = testJob("blocked", "blocked", { diagnostics: [
      { code: "scope_mismatch", message: "RAW_SECRET", retryAt: null },
      { code: "migration_requires_new_review", message: "RAW_SECRET", retryAt: null },
      { code: "UNKNOWN_SECRET" as SafeError["code"], message: "RAW_SECRET", retryAt: null }
    ] });
    renderJobs([job]); fireEvent.click(screen.getByRole("button", { name: `View details for cleanup job ${job.id}` }));
    expect(screen.getByText(/Reconnect the original account/)).toBeInTheDocument();
    expect(screen.getByText(safeMessages.migration_requires_new_review)).toBeInTheDocument();
    expect(screen.getByText("This action could not be completed. Review its status before retrying.")).toBeInTheDocument();
    expect(screen.getByText(/In-flight work may still have taken effect/)).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(/RAW_SECRET|UNKNOWN_SECRET/);
  });
});
