import { demoSearch, demoSnapshot, fixtureDescriptor, fixtureContext, fixtureRef, fixtureChatId, fixtureMessageId } from "../demo";
import { testId, testJob } from "../test/v2-fixtures";
import { uuid } from "../providers/identity";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { JobRecord, JobStatus } from "../types";
import { ImpactPanel } from "./ImpactPanel";

const timestamp = "2026-01-02T03:04:05.000Z";

const job = testJob;

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

    const queuedRow = screen.getByRole("group", { name: `Cleanup job ${testId("queued-job")}` });
    const runningRow = screen.getByRole("group", { name: `Cleanup job ${testId("running-job")}` });
    const retryRow = screen.getByRole("group", { name: `Cleanup job ${testId("rate-limited-job")}` });
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
      [testId("queued-job")],
      [testId("running-job")],
      [testId("rate-limited-job")],
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

    const completedRow = screen.getByRole("group", { name: `Cleanup job ${testId("completed-job")}` });
    const partialRow = screen.getByRole("group", { name: `Cleanup job ${testId("partial-job")}` });
    const failedRow = screen.getByRole("group", { name: `Cleanup job ${testId("failed-job")}` });
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

    const cancelledRow = screen.getByRole("group", { name: `Cleanup job ${testId("cancelled-job")}` });
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

describe("backend action presentation", () => {
  it("routes the supported sender intent but never aliases an unknown actor intent to sender deletion", async () => {
    const chat = (await demoSnapshot()).chats.find(c => c.id === fixtureChatId("-1003"))!;
    const selected = (await demoSearch({ query: "", conversations: [chat.ref], chatKinds: [], contentKinds: [], direction: "any", excludePinned: false, limit: 100 })).messages.slice(0, 1);
    const onSenderAction = vi.fn();
    render(<ImpactPanel {...panelProps([])} selected={selected} onSenderAction={onSenderAction} activeChat={{ ...chat,
      capabilities: { ...chat.capabilities, canDeleteBySender: false }, intents: [
        { actionId: "delete_by_sender", label: "Reviewed sender cleanup", requiresActor: true, descriptors: [fixtureDescriptor("delete_by_sender", "high")] },
        { actionId: "unknown_actor_action", label: "Unrecognized actor action", requiresActor: true, descriptors: [fixtureDescriptor("delete_by_sender", "high")] }
      ] }} />);
    fireEvent.click(screen.getByRole("button", { name: "Reviewed sender cleanup" }));
    expect(onSenderAction).toHaveBeenCalledWith(selected[0]);
    const unknown = screen.getByRole("button", { name: "Unrecognized actor action" });
    expect(unknown).toBeDisabled(); fireEvent.click(unknown);
    expect(onSenderAction).toHaveBeenCalledTimes(1);
  });
  it("uses executable descriptors even when role and metadata deny them", async () => {
    const chat = (await demoSnapshot()).chats[0];
    const onChatAction = vi.fn();
    render(<ImpactPanel {...panelProps([])} onChatAction={onChatAction} activeChat={{ ...chat,
      capabilities: { role: "member", canDeleteOthers: false, canClearForEveryone: false, canRemoveForSelf: false, canDeleteGroup: false, canDeleteBySender: false, canLeaveChat: false },
      intents: [{ actionId: "clear_history", label: "Backend-approved cleanup", requiresActor: false, descriptors: [fixtureDescriptor("clear_history", "high")] }]
    }} />);
    fireEvent.click(screen.getByRole("button", { name: "Backend-approved cleanup" }));
    expect(onChatAction).toHaveBeenCalledWith("clear_history");
    expect(screen.getByText("Remove for all participants")).toBeInTheDocument();
  });

  it("keeps unavailable and manual effects disabled despite owner metadata and hides absent intents", async () => {
    const chat = (await demoSnapshot()).chats[0];
    const descriptor = fixtureDescriptor("clear_history", "high");
    render(<ImpactPanel {...panelProps([])} activeChat={{ ...chat,
      capabilities: { role: "owner", canDeleteOthers: true, canClearForEveryone: true, canRemoveForSelf: true, canDeleteGroup: true, canDeleteBySender: true, canLeaveChat: true },
      intents: [
        { actionId: "clear_history", label: "Unavailable cleanup", requiresActor: false, descriptors: [{ ...descriptor, availability: "unavailable", unavailableReason: { code: "permission_changed", message: "RAW_SECRET", retryAt: null } }] },
        { actionId: "leave_chat", label: "Manual cleanup", requiresActor: false, descriptors: [{ ...descriptor, availability: "manual_only" }] }
      ]
    }} />);
    expect(screen.getByRole("button", { name: "Unavailable cleanup" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Manual cleanup" })).toBeDisabled();
    expect(screen.getByText(/Permission to perform this action has changed\./)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Delete group permanently/ })).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("RAW_SECRET");
  });

  it("shows every ordered compound effect and an independent critical destruction action", async () => {
    const chat = (await demoSnapshot()).chats[0];
    const cleanup = fixtureDescriptor("selected_messages", "high");
    const leave = fixtureDescriptor("leave_chat", "high");
    render(<ImpactPanel {...panelProps([])} activeChat={{ ...chat, intents: [
      { actionId: "leave_chat", label: "Cleanup then leave", requiresActor: false, descriptors: [cleanup, leave, fixtureDescriptor("remove_chat_for_self", "high")] },
      { actionId: "delete_group", label: "Destroy permanently", requiresActor: false, descriptors: [fixtureDescriptor("delete_group", "critical")] }
    ] }} />);
    const effects = screen.getByRole("list", { name: "Cleanup then leave effects" });
    expect(within(effects).getAllByRole("listitem").map(item => item.textContent)).toEqual([
      expect.stringContaining("Remove for all participants"), expect.stringContaining("Remove membership"), expect.stringContaining("Remove only for your account")
    ]);
    expect(screen.getByRole("button", { name: "Destroy permanently" })).toHaveClass("danger");
    expect(screen.getByText("Critical action")).toBeInTheDocument();
  });

  it("does not execute a compound intent when even one reviewed effect is unavailable", async () => {
    const chat = (await demoSnapshot()).chats[0];
    const onChatAction = vi.fn();
    render(<ImpactPanel {...panelProps([])} onChatAction={onChatAction} activeChat={{ ...chat, intents: [{
      actionId: "leave_chat", label: "Partially unavailable cleanup", requiresActor: false,
      descriptors: [fixtureDescriptor("selected_messages", "high"), { ...fixtureDescriptor("leave_chat", "high"), availability: "unavailable", unavailableReason: { code: "permission_changed", message: "RAW_SECRET", retryAt: null } }]
    }] }} />);
    const button = screen.getByRole("button", { name: "Partially unavailable cleanup" });
    expect(button).toBeDisabled(); fireEvent.click(button);
    expect(onChatAction).not.toHaveBeenCalled();
    expect(within(screen.getByRole("list", { name: /Partially unavailable cleanup effects/ })).getAllByRole("listitem")).toHaveLength(2);
  });
});
