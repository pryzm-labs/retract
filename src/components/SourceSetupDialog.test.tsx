import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "@retract/api";
import { demoSnapshot, fixtureContext } from "../demo";
import { providerKey } from "../providers/identity";
import { SourceSetupDialog } from "./SourceSetupDialog";
import type { DiscordImportStatus } from "../types";

afterEach(() => vi.restoreAllMocks());

const scope = { ...fixtureContext.scope, provider: providerKey("discord") };
const source = { scope, accountLabel: "Archive Owner", username: "owner", importedAt: "2026-09-15T12:00:00Z", warningCount: 0 };
const idleImport: DiscordImportStatus = { active: false, sources: [], progress: null, importScope: null, retryAvailable: false, failureCode: null, warningDetails: [] };

it("offers Telegram, native Discord import, and an existing imported source", async () => {
  const importArchive = vi.spyOn(api, "startDiscordImport").mockResolvedValue({ active: true, sources: [source], progress: { phase: "importing", inventoryEntries: 3, processedEntries: 2, totalBytes: 100, hashedBytes: 100, readBytes: 80, parsedRecords: 12, totalRecords: 20, committedItems: 10, committedBytes: 50, committedBatches: 1, warnings: 0 }, importScope: scope, retryAvailable: false, failureCode: null, warningDetails: [] });
  vi.spyOn(api, "discordImport").mockResolvedValue({ active: false, sources: [source], progress: { phase: "ready", inventoryEntries: 3, processedEntries: 3, totalBytes: 100, hashedBytes: 100, readBytes: 100, parsedRecords: 20, totalRecords: 20, committedItems: 20, committedBytes: 80, committedBatches: 2, warnings: 0 }, importScope: scope, retryAvailable: false, failureCode: null, warningDetails: [] });
  const selected = await demoSnapshot();
  vi.spyOn(api, "selectDiscordSource").mockResolvedValue(selected);
  const onSelected = vi.fn();

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelected={onSelected} onError={() => {}} />);
  expect(screen.getByRole("button", { name: /Telegram/ })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /Import Discord package/ }));
  await waitFor(() => expect(importArchive).toHaveBeenCalledWith(null));
  expect(await screen.findByText("Archive Owner")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /Archive Owner/ }));
  await waitFor(() => expect(onSelected).toHaveBeenCalledWith(selected));
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
  const selected = await demoSnapshot();
  vi.spyOn(api, "selectDiscordSource").mockResolvedValue(selected);
  const onSelected = vi.fn();

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelected={onSelected} onError={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: /Import Discord package/ }));

  await waitFor(() => expect(onSelected).toHaveBeenCalledWith(selected));
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

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelected={() => {}} onError={() => {}} />);
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

  render(<SourceSetupDialog context={null} telegramConfigured={false} required onClose={() => {}} onTelegram={() => {}} onSelected={() => {}} onError={() => {}} />);

  expect(await screen.findByRole("button", { name: "Retry failed import" })).toBeVisible();
  expect(screen.getByText("998 staged records · not yet available")).toBeVisible();
});
