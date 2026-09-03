import { fixtureContext, fixtureRef, fixtureChatId, fixtureMessageId } from "../demo";
import { testId, testJob } from "../test/v2-fixtures";
import { uuid } from "../providers/identity";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "@retract/api";
import type { ConnectionSettings, SaveConnectionSettingsResult } from "../types";
import { ConnectionSettingsDialog } from "./ConnectionSettingsDialog";

const bundledSettings: ConnectionSettings = {
  setupComplete: false,
  tdlibPath: "/Applications/Retract.app/Contents/Resources/lib/libtdjson.dylib",
  detectedTdlibPath: "/Applications/Retract.app/Contents/Resources/lib/libtdjson.dylib",
  bundledTdlibAvailable: true,
  apiId: null,
  apiHashConfigured: false,
  useTestDc: false,
  environmentOverrides: [],
  configurationError: null,
  supportedTdlibVersion: "1.8.64"
};

describe("ConnectionSettingsDialog", () => {
  afterEach(() => vi.restoreAllMocks());

  it("requires real Telegram credentials without offering fixture mode", () => {
    render(<ConnectionSettingsDialog settings={bundledSettings} required onClose={vi.fn()} onSaved={vi.fn()} />);

    expect(screen.getByRole("heading", { name: "Connect Telegram" })).toBeInTheDocument();
    expect(screen.queryByText("Safe demo")).not.toBeInTheDocument();
    expect(screen.getByText("TDLib 1.8.64 included")).toBeInTheDocument();
    expect(screen.getByText("READY")).toBeInTheDocument();
    expect(screen.getByLabelText("Telegram API ID")).toBeInTheDocument();
    expect(screen.getByLabelText("Telegram API hash")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save settings" })).toBeDisabled();
  });

  it("saves only real Telegram connection fields", async () => {
    const liveResult: SaveConnectionSettingsResult = {
      connectionSettings: { ...bundledSettings, setupComplete: true },
      snapshot: {
        context: fixtureContext,
        identity: { state: "ready" },
        catalog: { phase: "ready", total: 0, processed: 0 },
        legacyHistory: [],
        accountLabel: "Telegram account",
        modeReason: "Connected locally",
        chats: [],
        recentJobs: [],
        safetyNotice: "Deletion plans are capability checked.",
        auth: { stage: "ready" }
      }
    };
    const save = vi.spyOn(api, "saveConnectionSettings").mockResolvedValue(liveResult);
    render(<ConnectionSettingsDialog settings={bundledSettings} required onClose={vi.fn()} onSaved={vi.fn()} />);

    fireEvent.change(screen.getByLabelText("Telegram API ID"), { target: { value: "12345678" } });
    fireEvent.change(screen.getByLabelText("Telegram API hash"), { target: { value: "0123456789abcdef0123456789abcdef" } });
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));

    await waitFor(() => expect(save).toHaveBeenCalledWith({
      tdlibPath: bundledSettings.tdlibPath,
      apiId: 12345678,
      apiHash: "0123456789abcdef0123456789abcdef",
      useTestDc: false
    }, null));
  });

  it("does not publish a delayed save after a genuine StrictMode dialog unmount", async () => {
    const snapshot = await api.snapshot(fixtureContext);
    let resolveSave!: (value: SaveConnectionSettingsResult) => void;
    const save = vi.spyOn(api, "saveConnectionSettings").mockReturnValue(new Promise(resolve => { resolveSave = resolve; }));
    const onSaved = vi.fn();
    const view = render(<StrictMode><ConnectionSettingsDialog context={fixtureContext} settings={{ ...bundledSettings, apiId: 12345678, apiHashConfigured: true }} required onClose={vi.fn()} onSaved={onSaved} /></StrictMode>);
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    expect(save).toHaveBeenCalledTimes(1);
    view.unmount();
    await act(async () => resolveSave({ snapshot, connectionSettings: bundledSettings }));
    expect(onSaved).not.toHaveBeenCalled();
  });
});
