import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";
import { fixtureApi } from "../api.fixture";
import { fixtureContext, fixtureDescriptor, fixtureRef } from "../demo";
import { api } from "@retract/api";
import type { PlanOperation } from "../types";
import { readFileSync } from "node:fs";

const styles = readFileSync("src/styles.css", "utf8");

describe("reviewed backend confirmation", () => {
  it.each<PlanOperation>(["clear_history_and_leave", "delete_all_messages_and_leave", "leave_chat"])("does not promise missing cleanup or self-removal for %s", async operation => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    const cleanup = operation === "clear_history_and_leave" ? "clear_history" : "selected_messages";
    for (const actions of [["leave_chat"], [cleanup, "leave_chat"], ["leave_chat", "remove_chat_for_self"], ["remove_chat_for_self", cleanup, "leave_chat"]] as PlanOperation[][]) {
      const view = render(<ConfirmDialog plan={{ ...plan, operation, summary: { ...plan.summary, deleteForEveryone: 1 },
        steps: actions.map(action => ({ descriptor: fixtureDescriptor(action), targets: plan.targets }))
      }} busy={false} onClose={vi.fn()} onConfirm={vi.fn()} />);
      expect(screen.getByRole("heading", { name: "Review the frozen effects?" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Confirm reviewed effects" })).toBeInTheDocument();
      if (operation !== "clear_history_and_leave") expect(screen.getByText("protected skipped")).toBeInTheDocument();
      view.unmount();
    }
  });

  it("does not describe one-message deletion as whole-history clearing", async () => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    render(<ConfirmDialog plan={{ ...plan, steps: [{ descriptor: fixtureDescriptor("selected_messages"), targets: plan.targets }] }} busy={false} onClose={vi.fn()} onConfirm={vi.fn()} />);
    expect(screen.getByRole("heading", { name: "Review the frozen effects?" })).toBeInTheDocument();
    expect(screen.getByRole("dialog")).not.toHaveTextContent("Every message Telegram permits");
  });

  it.each<PlanOperation>(["clear_history_and_leave", "delete_all_messages_and_leave", "leave_chat"])("preserves familiar copy only for the complete ordered %s shape", async operation => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    const actions: PlanOperation[] = operation === "clear_history_and_leave"
      ? ["clear_history", "leave_chat", "remove_chat_for_self"]
      : ["selected_messages", "selected_messages", "leave_chat", "remove_chat_for_self"];
    const steps = actions.map(action => ({ descriptor: fixtureDescriptor(action), targets: plan.targets }));
    const props = { plan: { ...plan, operation, steps }, busy: false, onClose: vi.fn(), onConfirm: vi.fn() };
    const view = render(<ConfirmDialog {...props} />);
    expect(screen.queryByRole("heading", { name: "Review the frozen effects?" })).not.toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Ordered plan effects" })).getAllByRole("listitem")).toHaveLength(steps.length);
    view.rerender(<ConfirmDialog {...props} plan={{ ...props.plan, steps: steps.map((step, index) => index === 0 ? { ...step, descriptor: { ...step.descriptor, effect: "manual_or_unknown" } } : step) }} />);
    expect(screen.getByRole("heading", { name: "Review the frozen effects?" })).toBeInTheDocument();
  });

  it("keeps every batch and final controls in a viewport-bounded scrolling dialog", async () => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    const onConfirm = vi.fn();
    render(<><style>{styles}</style><ConfirmDialog plan={{ ...plan, operation: "selected_messages",
      confirmation: { tier: "high", acknowledgementRequired: true, exactText: "REVIEW", ownerAuthRequired: false },
      steps: Array.from({ length: 250 }, () => ({ descriptor: fixtureDescriptor("selected_messages"), targets: plan.targets }))
    }} busy={false} onClose={vi.fn()} onConfirm={onConfirm} /></>);
    const dialog = screen.getByRole("dialog");
    // jsdom verifies the applied scrolling contract, not browser geometry.
    expect(getComputedStyle(dialog).overflow).toBe("auto");
    expect(getComputedStyle(dialog).maxHeight).toMatch(/calc\(100d?vh - 48px\)/);
    expect(within(screen.getByRole("list", { name: "Ordered plan effects" })).getAllByRole("listitem")).toHaveLength(250);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "REVIEW" } });
    fireEvent.click(screen.getByRole("checkbox"));
    const confirm = dialog.querySelector<HTMLButtonElement>(".confirm-button")!;
    confirm.focus();
    expect(confirm).toHaveFocus();
    fireEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledWith(true, "REVIEW");
  });

  it("does not enable confirmation for a reviewed but unavailable effect", async () => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    render(<ConfirmDialog plan={{ ...plan, confirmation: { tier: "low", acknowledgementRequired: false, ownerAuthRequired: false, exactText: null },
      steps: plan.steps.map(step => ({ ...step, descriptor: { ...step.descriptor, availability: "unavailable", unavailableReason: { code: "permission_changed", message: "RAW_SECRET", retryAt: null } } }))
    }} busy={false} onClose={vi.fn()} onConfirm={vi.fn()} />);
    expect(document.querySelector(".confirm-button")).toBeDisabled();
    expect(screen.getByText(/Permission to perform this action has changed\./)).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("RAW_SECRET");
  });
  it("does not let an operation display label override the reviewed effect", async () => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    render(<ConfirmDialog plan={{ ...plan, operation: "delete_group", steps: [{ descriptor: fixtureDescriptor("remove_chat_for_self", "low"), targets: plan.targets }],
      confirmation: { tier: "low", acknowledgementRequired: true, ownerAuthRequired: false, exactText: null }
    }} busy={false} onClose={vi.fn()} onConfirm={vi.fn()} />);
    const dialog = screen.getByRole("dialog");
    expect(dialog).not.toHaveTextContent(/remove every member|dissolve this group|Delete .*for everyone/);
    expect(within(dialog).getByText("Remove only for your account")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Confirm reviewed effects" })).toBeInTheDocument();
  });
  it("receives complete compound effects through the shared fixture API", async () => {
    await fixtureApi.resetFixtures();
    const ref = fixtureRef("conversation", "-1002");
    const catalog = await api.intents([ref], fixtureContext);
    expect(catalog.find(intent => intent.actionId === "leave_chat")?.descriptors.map(d => d.effect)).toEqual([
      "removed_for_all_participants", "membership_removed", "removed_for_current_account_only"
    ]);
    const plan = await api.prepareChatAction(ref, "leave_chat", fixtureContext);
    expect(plan.steps.map(step => step.descriptor.effect)).toEqual([
      "removed_for_all_participants", "membership_removed", "removed_for_current_account_only"
    ]);
  });
  it("obeys exact backend text and tier instead of projected title/tier and shows ordered effects", async () => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    const onConfirm = vi.fn();
    const ref = fixtureRef("conversation", "-1001");
    render(<ConfirmDialog plan={{ ...plan, chatTitle: "Display title", confirmationTier: "low",
      confirmation: { tier: "critical", acknowledgementRequired: true, ownerAuthRequired: true, exactText: "EXACT reviewed title" },
      steps: [fixtureDescriptor("selected_messages", "high"), fixtureDescriptor("leave_chat", "high"), fixtureDescriptor("remove_chat_for_self", "high")].map(descriptor => ({ descriptor, targets: [ref] }))
    }} busy={false} onClose={vi.fn()} onConfirm={onConfirm} />);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveClass("is-critical");
    const effects = screen.getByRole("list", { name: "Ordered plan effects" });
    expect(within(effects).getAllByRole("listitem").map(item => item.textContent)).toEqual([
      expect.stringContaining("Remove for all participants"), expect.stringContaining("Remove membership"), expect.stringContaining("Remove only for your account")
    ]);
    const input = screen.getByRole("textbox", { name: /EXACT reviewed title/ });
    fireEvent.change(input, { target: { value: "Display title" } });
    fireEvent.click(screen.getByRole("checkbox"));
    expect(dialog.querySelector(".confirm-button")).toBeDisabled();
    fireEvent.change(input, { target: { value: "EXACT reviewed title " } });
    expect(dialog.querySelector(".confirm-button")).toBeDisabled();
    fireEvent.change(input, { target: { value: "EXACT reviewed title" } });
    fireEvent.click(dialog.querySelector(".confirm-button")!);
    expect(onConfirm).toHaveBeenCalledWith(true, "EXACT reviewed title");
  });

  it("does not invent acknowledgement, text or owner-auth requirements", async () => {
    await fixtureApi.resetFixtures();
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "clear_history", fixtureContext);
    render(<ConfirmDialog plan={{ ...plan, confirmationTier: "critical", confirmation: { tier: "low", acknowledgementRequired: false, ownerAuthRequired: false, exactText: null } }} busy={false} onClose={vi.fn()} onConfirm={vi.fn()} />);
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByText(/macOS will show/)).not.toBeInTheDocument();
    expect(document.querySelector(".confirm-button")).toBeEnabled();
  });
});
