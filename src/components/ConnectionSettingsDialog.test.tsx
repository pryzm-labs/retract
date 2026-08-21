import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import type { ConnectionSettings, SaveConnectionSettingsResult } from "../types";
import { ConnectionSettingsDialog } from "./ConnectionSettingsDialog";

const bundledSettings: ConnectionSettings = {
  setupComplete: false,
  runtimeMode: "demo",
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

  it("uses the bundled TDLib without asking an end user for a library path", () => {
    render(<ConnectionSettingsDialog settings={bundledSettings} required onClose={vi.fn()} onSaved={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: /Connect Telegram/ }));

    expect(screen.getByText("TDLib 1.8.64 included")).toBeInTheDocument();
    expect(screen.getByText("READY")).toBeInTheDocument();
    expect(screen.getByLabelText("Telegram API ID")).toHaveAttribute("type", "text");
    expect(screen.queryByPlaceholderText("/absolute/path/to/libtdjson.dylib")).not.toBeInTheDocument();
  });

  it("applies Safe demo without requesting a process restart", async () => {
    const result: SaveConnectionSettingsResult = {
      connectionSettings: { ...bundledSettings, setupComplete: true },
      snapshot: {
        runtimeMode: "demo",
        accountLabel: "Local demo",
        modeReason: "Disposable fixtures",
        chats: [],
        recentJobs: [],
        safetyNotice: "Demo only",
        auth: { stage: "ready" }
      }
    };
    const save = vi.spyOn(api, "saveConnectionSettings").mockResolvedValue(result);
    const onSaved = vi.fn();
    render(<ConnectionSettingsDialog settings={bundledSettings} required onClose={vi.fn()} onSaved={onSaved} />);

    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));

    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(result));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ runtimeMode: "demo" }));
    expect(screen.getByText("Settings applied.")).toBeInTheDocument();
  });
});
