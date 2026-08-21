import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { api } from "./api";
import type { AppSnapshot, ChatSummary, JobRecord } from "./types";

describe("Retract desktop UI", () => {
  beforeEach(async () => {
    await api.resetDemo();
  });

  afterEach(() => vi.restoreAllMocks());

  it("never exposes fixture controls in the end-user shell", async () => {
    render(<App />);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
    expect(screen.queryByText(/Safe demo/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Reset demo fixtures/i })).not.toBeInTheDocument();
  });

  it("leaves the password gate before the full Telegram catalog finishes loading", async () => {
    const demoSnapshot = await api.snapshot();
    const demoSettings = await api.connectionSettings();
    const waiting: AppSnapshot = {
      ...demoSnapshot,
      runtimeMode: "live",
      chats: [],
      auth: { stage: "waiting_for_password", hint: "account password hint" }
    };
    const ready: AppSnapshot = {
      ...demoSnapshot,
      runtimeMode: "live",
      auth: { stage: "ready" }
    };
    let finishCatalog!: (snapshot: AppSnapshot) => void;
    const catalog = new Promise<AppSnapshot>((resolve) => { finishCatalog = resolve; });

    vi.spyOn(api, "bootstrapSnapshot").mockResolvedValueOnce(waiting);
    vi.spyOn(api, "snapshot").mockImplementation(() => catalog);
    vi.spyOn(api, "connectionSettings").mockResolvedValue(demoSettings);
    vi.spyOn(api, "catalogProgress").mockResolvedValue({
      phase: "loading",
      total: 531,
      processed: 128
    });
    vi.spyOn(api, "submitAuth").mockResolvedValue();
    vi.spyOn(api, "authSnapshot").mockResolvedValue({ stage: "ready" });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "Two-step verification" })).toBeInTheDocument();
    expect(screen.getByText(/Telegram approved this device/)).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Two-step verification password"), {
      target: { value: "correct horse battery staple" }
    });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));

    expect(await screen.findByRole("heading", { name: "Preparing your workspace" })).toBeInTheDocument();
    expect(await screen.findByText("128 of 531 chats processed")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Two-step verification" })).not.toBeInTheDocument();
    expect(api.submitAuth).toHaveBeenCalledWith("submit_password", "correct horse battery staple");

    finishCatalog(ready);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
  });

  it("makes role and effective admin authority visible", async () => {
    render(<App />);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Retract app logo" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Design Team/ }));
    expect(await screen.findByText("CHAT AUTHORITY")).toBeInTheDocument();
    expect(screen.getAllByText("Owner").length).toBeGreaterThan(0);
    expect(screen.getByText("Permanently delete group")).toBeInTheDocument();
  });

  it("makes admin-wide message cleanup explicit before leave-and-remove", async () => {
    render(<App />);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Volunteer Archive/ }));
    fireEvent.click(await screen.findByRole("button", { name: /Delete all possible history & leave group/ }));

    expect(await screen.findByRole("heading", { name: "Delete all possible history and leave “Volunteer Archive”?" })).toBeInTheDocument();
    expect(screen.getByText(/from every participant in this chat/)).toBeInTheDocument();
    expect(screen.getByText(/Rejected or protected messages will be reported/)).toBeInTheDocument();
    expect(document.querySelector(".plan-binding")?.textContent).toMatch(/every enumerated message ID and this chat/);
  });

  it("favors complete history cleanup when an admin leaves", async () => {
    render(<App />);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Neighborhood Exchange/ }));
    fireEvent.click(await screen.findByRole("button", { name: /Clear all history & leave group/ }));

    expect(await screen.findByRole("heading", { name: "Clear all history and leave “Neighborhood Exchange”?" })).toBeInTheDocument();
    expect(screen.getByText(/complete history for everyone before leaving/)).toBeInTheDocument();
  });

  it("offers self-only removal for an empty DM when full revocation is unavailable", async () => {
    const now = new Date().toISOString();
    const queuedRemoval: JobRecord = {
      id: "queued-removal",
      planId: "self-only-plan",
      operation: "remove_chat_for_self",
      targetChatIds: [304],
      status: "queued",
      total: 0,
      deleted: 0,
      skipped: 0,
      failed: 0,
      nextBatch: 0,
      retryAfterSeconds: null,
      errorCodes: [],
      createdAt: now,
      updatedAt: now
    };
    vi.spyOn(api, "execute").mockResolvedValueOnce(queuedRemoval);

    render(<App />);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Empty invite/ }));

    expect(screen.getByText("Delete history and remove for me")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove chat from my list" }));

    expect(await screen.findByRole("heading", { name: "Remove “Empty invite” from your chat list?" })).toBeInTheDocument();
    expect(screen.getByText(/other participant or group members keep their copies/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: /deletes only my history and chat-list entry/i }));
    fireEvent.click(screen.getByRole("button", { name: "Remove chat for me" }));

    expect(await screen.findByText("Waiting for Telegram to finish removing this chat…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Removing chat…" })).toBeDisabled();
    expect(screen.getByText("Removing…").closest("button")).toBeDisabled();
  });

  it("hides a completed chat removal without re-adding a stale targeted refresh", async () => {
    const emptyChat = (await api.snapshot()).chats.find((chat) => chat.id === 304)!;
    const now = new Date().toISOString();
    vi.spyOn(api, "execute").mockResolvedValueOnce({
      id: "completed-removal",
      planId: "self-only-plan",
      operation: "remove_chat_for_self",
      targetChatIds: [304],
      status: "completed",
      total: 0,
      deleted: 0,
      skipped: 0,
      failed: 0,
      nextBatch: 0,
      retryAfterSeconds: null,
      errorCodes: [],
      createdAt: now,
      updatedAt: now
    });
    const targetedRefresh = vi.spyOn(api, "refreshChats").mockResolvedValueOnce([emptyChat]);

    render(<App />);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Empty invite/ }));
    fireEvent.click(screen.getByRole("button", { name: "Remove chat from my list" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /deletes only my history and chat-list entry/i }));
    fireEvent.click(screen.getByRole("button", { name: "Remove chat for me" }));

    await waitFor(() => expect(targetedRefresh).toHaveBeenCalledWith([304]));
    expect(screen.queryByRole("button", { name: /Empty invite/ })).not.toBeInTheDocument();
  });

  it("offers a group-only action that preserves membership and other messages", async () => {
    render(<App />);
    expect(await screen.findByText("Search every chat")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Volunteer Archive/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete all my messages" }));

    expect(await screen.findByRole("heading", { name: "Delete all your messages from “Volunteer Archive”?" })).toBeInTheDocument();
    expect(screen.getByText(/Your membership and every other participant’s messages remain/)).toBeInTheDocument();
    expect(document.querySelector(".plan-binding")?.textContent).toMatch(/is frozen to your message IDs in this chat/);
  });

  it("shows an impact review before any deletion call", async () => {
    render(<App />);
    await screen.findByText("Search every chat");
    const message = await screen.findByText("Passport scan for the apartment application");
    fireEvent.click(message);
    const review = screen.getByRole("button", { name: /Review deletion/ });
    await waitFor(() => expect(review).toBeEnabled());
    fireEvent.click(review);
    expect(await screen.findByText("Delete 1 message for everyone?")).toBeInTheDocument();
    expect(screen.getByText(/accepted Telegram deletions cannot be undone/)).toBeInTheDocument();
  });

  it("keeps new selections actionable while a completed cleanup refreshes in the background", async () => {
    let finishRefresh!: (chats: ChatSummary[]) => void;
    const refresh = new Promise<ChatSummary[]>((resolve) => { finishRefresh = resolve; });
    const targetedRefresh = vi.spyOn(api, "refreshChats").mockImplementationOnce(() => refresh);
    const globalRefresh = vi.spyOn(api, "snapshot");

    render(<App />);
    await screen.findByText("Search every chat");
    fireEvent.click(await screen.findByText("Passport scan for the apartment application"));
    fireEvent.click(screen.getByRole("button", { name: /Review deletion/ }));
    expect(await screen.findByText("Delete 1 message for everyone?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: /accepted Telegram deletions cannot be undone/ }));
    fireEvent.click(screen.getByRole("button", { name: "Delete for everyone" }));

    expect(await screen.findByText("Syncing cleanup…")).toBeInTheDocument();
    expect(targetedRefresh).toHaveBeenCalledTimes(1);
    expect(globalRefresh).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Project Cedar launch credentials moved to the vault."));
    expect(screen.getByRole("button", { name: /Review deletion/ })).toBeEnabled();

    finishRefresh([]);
    await waitFor(() => expect(screen.queryByText("Syncing cleanup…")).not.toBeInTheDocument());
  });

  it("shortlists chats with no reply and confirmed-empty chats", async () => {
    render(<App />);
    await screen.findByText("Search every chat");

    fireEvent.click(screen.getByRole("button", { name: /^No reply sent 1$/ }));
    expect(await screen.findByRole("button", { name: /Prize Support/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Maya Chen/ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^Empty 1$/ }));
    expect(await screen.findByRole("button", { name: /Empty invite/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Prize Support/ })).not.toBeInTheDocument();
  });

  it("runs a local privacy scan and labels why a message matched", async () => {
    render(<App />);
    await screen.findByText("Search every chat");
    fireEvent.click(screen.getByRole("button", { name: "Privacy scan" }));
    expect(await screen.findByText(/Backup contact person@example.com/)).toBeInTheDocument();
    expect(screen.getByText("Email")).toBeInTheDocument();
    expect(screen.getByText("Crypto wallet")).toBeInTheDocument();
    expect(screen.getByText(/pixels inside photos and external copies are not inspected/i)).toBeInTheDocument();
  });

  it("opens end-user connection settings from the sidebar", async () => {
    render(<App />);
    await screen.findByText("Search every chat");
    fireEvent.click(screen.getByRole("button", { name: "Open connection settings" }));
    expect(await screen.findByRole("heading", { name: "Telegram connection" })).toBeInTheDocument();
    expect(screen.getByText("TDLib library")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save settings" })).toBeDisabled();
  });
});
