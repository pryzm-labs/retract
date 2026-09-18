import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "@retract/api";
import { fixtureContext } from "../demo";
import { providerKey } from "../providers/identity";
import { SourceSetupDialog } from "./SourceSetupDialog";
import type { DiscordImportStatus } from "../types";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const scope = { ...fixtureContext.scope, provider: providerKey("discord") };
const source = { scope, accountLabel: "Archive Owner", username: "owner", importedAt: "2026-09-15T12:00:00Z", warningCount: 0 };
const idleImport: DiscordImportStatus = { active: false, sources: [], progress: null, importScope: null, retryAvailable: false, failureCode: null, warningDetails: [] };

it("offers Telegram, native Discord import, and an existing imported source", async () => {
  const importArchive = vi.spyOn(api, "startDiscordImport").mockResolvedValue({ active: true, sources: [source], progress: { phase: "importing", inventoryEntries: 3, processedEntries: 2, totalBytes: 100, hashedBytes: 100, readBytes: 80, parsedRecords: 12, totalRecords: 20, committedItems: 10, committedBytes: 50, committedBatches: 1, warnings: 0 }, importScope: scope, retryAvailable: false, failureCode: null, warningDetails: [] });
  vi.spyOn(api, "discordImport").mockResolvedValue({ active: false, sources: [source], progress: { phase: "ready", inventoryEntries: 3, processedEntries: 3, totalBytes: 100, hashedBytes: 100, readBytes: 100, parsedRecords: 20, totalRecords: 20, committedItems: 20, committedBytes: 80, committedBatches: 2, warnings: 0 }, importScope: scope, retryAvailable: false, failureCode: null, warningDetails: [] });
  const onSelect = vi.fn().mockResolvedValue(undefined);

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelect={onSelect} onError={() => {}} />);
  expect(screen.getByRole("button", { name: /Telegram/ })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /Import Discord package/ }));
  await waitFor(() => expect(importArchive).toHaveBeenCalledWith(null));
  expect(await screen.findByText("Archive Owner")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /Archive Owner/ }));
  await waitFor(() => expect(onSelect).toHaveBeenCalledWith(source));
});

it("shows source discovery progress until imported Discord accounts are available", async () => {
  let resolveStatus!: (value: DiscordImportStatus) => void;
  vi.spyOn(api, "discordImport").mockImplementation(() => new Promise(resolve => { resolveStatus = resolve; }));

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelect={async () => {}} onError={() => {}} />);

  expect(screen.getByRole("status", { name: "Imported Discord accounts" })).toHaveTextContent("Loading imported accounts…");
  expect(screen.queryByText("No imported Discord accounts yet.")).not.toBeInTheDocument();

  resolveStatus({ ...idleImport, sources: [source] });

  expect(await screen.findByRole("button", { name: /Archive Owner/ })).toBeVisible();
  expect(screen.queryByText("Loading imported accounts…")).not.toBeInTheDocument();
});

it("shows a retryable source discovery error instead of an empty account list", async () => {
  const status = vi.spyOn(api, "discordImport")
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValueOnce({ ...idleImport, sources: [source] });
  const onError = vi.fn();

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelect={async () => {}} onError={onError} />);

  expect(await screen.findByText("Couldn’t load imported accounts.")).toBeVisible();
  expect(onError).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole("button", { name: "Retry loading imported accounts" }));

  expect(await screen.findByRole("button", { name: /Archive Owner/ })).toBeVisible();
  expect(status).toHaveBeenCalledTimes(2);
});

it("opens the newly ready Discord source after import", async () => {
  vi.spyOn(api, "discordImport").mockResolvedValue(idleImport);
  vi.spyOn(api, "startDiscordImport").mockResolvedValue({
    active: false,
    sources: [source],
    progress: { phase: "ready", inventoryEntries: 3, processedEntries: 3, totalBytes: 100, hashedBytes: 200, readBytes: 100, parsedRecords: 20, totalRecords: 20, committedItems: 20, committedBytes: 80, committedBatches: 2, warnings: 0 },
    importScope: scope,
    retryAvailable: false,
    failureCode: null,
    warningDetails: []
  } as DiscordImportStatus);
  const onSelect = vi.fn().mockResolvedValue(undefined);

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelect={onSelect} onError={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: /Import Discord package/ }));

  await waitFor(() => expect(onSelect).toHaveBeenCalledWith(source));
});

it("ignores an obsolete import poll once source selection starts", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  let rejectPoll!: (reason: Error) => void;
  const pendingPoll = new Promise<DiscordImportStatus>((_, reject) => { rejectPoll = reject; });
  const active = {
    ...idleImport,
    active: true,
    sources: [source],
    progress: { phase: "importing", inventoryEntries: 3, processedEntries: 2, totalBytes: 100, hashedBytes: 100, readBytes: 80, parsedRecords: 12, totalRecords: 20, committedItems: 10, committedBytes: 50, committedBatches: 1, warnings: 0 },
    importScope: scope
  } as DiscordImportStatus;
  const importStatus = vi.spyOn(api, "discordImport")
    .mockResolvedValueOnce(active)
    .mockImplementationOnce(() => pendingPoll);
  const onSelect = vi.fn(() => new Promise<void>(() => {}));
  const onError = vi.fn();

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelect={onSelect} onError={onError} />);
  await screen.findByRole("button", { name: /Archive Owner/ });
  await vi.advanceTimersByTimeAsync(400);
  await waitFor(() => expect(importStatus).toHaveBeenCalledTimes(2));
  fireEvent.click(screen.getByRole("button", { name: /Archive Owner/ }));
  rejectPoll(new Error("This action belongs to a different account or source."));
  await vi.advanceTimersByTimeAsync(0);

  expect(onSelect).toHaveBeenCalledWith(source);
  expect(onError).not.toHaveBeenCalled();
});

it("explains a failed staged import and exposes its safe warnings", async () => {
  vi.spyOn(api, "discordImport").mockResolvedValue(idleImport);
  vi.spyOn(api, "startDiscordImport").mockResolvedValue({
    active: false,
    sources: [],
    progress: { phase: "failed", inventoryEntries: 1_426, processedEntries: 900, totalBytes: 100, hashedBytes: 100, readBytes: 80, parsedRecords: 1_000, totalRecords: null, committedItems: 998, committedBytes: 50, committedBatches: 2, warnings: 2 },
    importScope: scope,
    retryAvailable: true,
    failureCode: "invalid_archive",
    warningDetails: [{ code: "unknown_conversation_kind", count: 2 }]
  } as DiscordImportStatus);
  const retryImport = vi.spyOn(api, "retryDiscordImport").mockResolvedValue({
    ...idleImport,
    active: true,
    importScope: scope,
    progress: { phase: "importing", inventoryEntries: 1_426, processedEntries: 2, totalBytes: 100, hashedBytes: 100, readBytes: 0, parsedRecords: 0, totalRecords: null, committedItems: 0, committedBytes: 0, committedBatches: 0, warnings: 0 }
  });

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelect={async () => {}} onError={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: /Import Discord package/ }));

  expect(await screen.findByText("998 staged records · not yet available")).toBeVisible();
  expect(screen.getByText("2 conversations have an unverified type and will remain searchable after a successful import.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Retry failed import" })).toBeVisible();
  expect(screen.queryByText("1,000 messages")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Retry failed import" }));
  await waitFor(() => expect(retryImport).toHaveBeenCalledWith(null));
});

it("restores a failed import when the source dialog is reopened", async () => {
  vi.spyOn(api, "discordImport").mockResolvedValue({
    active: false,
    sources: [],
    progress: { phase: "failed", inventoryEntries: 1_426, processedEntries: 900, totalBytes: 100, hashedBytes: 100, readBytes: 80, parsedRecords: 0, totalRecords: null, committedItems: 998, committedBytes: 50, committedBatches: 2, warnings: 2 },
    importScope: scope,
    retryAvailable: true,
    failureCode: "invalid_archive",
    warningDetails: [{ code: "unknown_conversation_kind", count: 2 }]
  });

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelect={async () => {}} onError={() => {}} />);

  expect(await screen.findByRole("button", { name: "Retry failed import" })).toBeVisible();
  expect(screen.getByText("998 staged records · not yet available")).toBeVisible();
});
