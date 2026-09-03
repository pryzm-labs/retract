import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";
import { fixtureApi } from "../api.fixture";
import { fixtureContext, fixtureDescriptor, fixtureRef } from "../demo";
import { api } from "@retract/api";

describe("reviewed backend confirmation", () => {
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
