import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "@retract/api";
import { demoSnapshot, fixtureContext } from "../demo";
import { providerKey } from "../providers/identity";
import { SourceSetupDialog } from "./SourceSetupDialog";

afterEach(() => vi.restoreAllMocks());

it("offers Telegram, native Discord import, and an existing imported source", async () => {
  const scope = { ...fixtureContext.scope, provider: providerKey("discord") };
  const source = { scope, accountLabel: "Archive Owner", username: "owner", importedAt: "2026-09-15T12:00:00Z", warningCount: 0 };
  vi.spyOn(api, "discordSources").mockResolvedValue([source]);
  const importArchive = vi.spyOn(api, "startDiscordImport").mockResolvedValue({ active: true, sources: [source], progress: { phase: "importing", inventoryEntries: 3, processedEntries: 2, totalBytes: 100, hashedBytes: 100, readBytes: 80, parsedRecords: 12, totalRecords: 20, committedItems: 10, committedBytes: 50, committedBatches: 1, warnings: 0 } });
  vi.spyOn(api, "discordImport").mockResolvedValue({ active: false, sources: [source], progress: { phase: "ready", inventoryEntries: 3, processedEntries: 3, totalBytes: 100, hashedBytes: 100, readBytes: 100, parsedRecords: 20, totalRecords: 20, committedItems: 20, committedBytes: 80, committedBatches: 2, warnings: 0 } });
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
