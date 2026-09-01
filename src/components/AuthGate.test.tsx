import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@retract/api";
import type { AuthCommand } from "../api-contract";
import type { AuthSnapshot } from "../types";
import { AuthGate } from "./AuthGate";

vi.mock("@retract/api", () => ({
  api: {
    requestQrAuth: vi.fn(),
    submitAuth: vi.fn()
  }
}));

const authCases = [
  ["waiting_for_phone", "Phone number", "submit_phone"],
  ["waiting_for_email_address", "Email address", "submit_email_address"],
  ["waiting_for_email_code", "Email code", "submit_email_code"],
  ["waiting_for_code", "Telegram sign-in code", "submit_code"],
  ["waiting_for_password", "Two-step verification password", "submit_password"]
] as const satisfies ReadonlyArray<readonly [AuthSnapshot["stage"], string, AuthCommand]>;

const secretCases = [
  ["waiting_for_email_code", "Email code"],
  ["waiting_for_code", "Telegram sign-in code"],
  ["waiting_for_password", "Two-step verification password"]
] as const satisfies ReadonlyArray<readonly [AuthSnapshot["stage"], string]>;

const nonSecretCases = [
  ["waiting_for_phone", "Phone number"],
  ["waiting_for_email_address", "Email address"]
] as const satisfies ReadonlyArray<readonly [AuthSnapshot["stage"], string]>;

const nonPhoneStages: AuthSnapshot["stage"][] = [
  "initializing",
  "waiting_for_email_address",
  "waiting_for_email_code",
  "waiting_for_code",
  "waiting_for_password",
  "waiting_for_other_device",
  "ready",
  "logging_out",
  "closed",
  "error"
];

function renderGate(auth: AuthSnapshot, onRefresh = vi.fn().mockResolvedValue(undefined)) {
  const view = render(
    <AuthGate auth={auth} onRefresh={onRefresh} onOpenSettings={vi.fn()} />
  );
  return { ...view, onRefresh };
}

describe("Telegram authentication gate", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.requestQrAuth).mockResolvedValue(undefined);
    vi.mocked(api.submitAuth).mockResolvedValue(undefined);
  });

  it("maps every credential stage to its exact label and auth command", async () => {
    for (const [stage, label, command] of authCases) {
      const { onRefresh, unmount } = renderGate({ stage });

      const input = screen.getByLabelText(label);
      fireEvent.change(input, { target: { value: "synthetic-value" } });
      fireEvent.submit(input.closest("form")!);

      await waitFor(() => expect(api.submitAuth).toHaveBeenCalledWith(command, "synthetic-value"));
      expect(api.submitAuth).toHaveBeenCalledTimes(1);
      expect(onRefresh).toHaveBeenCalledTimes(1);
      expect(vi.mocked(api.submitAuth).mock.invocationCallOrder[0])
        .toBeLessThan(onRefresh.mock.invocationCallOrder[0]);

      unmount();
      vi.clearAllMocks();
    }
  });

  it("clears secret fields and shows normalized errors after rejected submissions", async () => {
    for (const [stage, label] of secretCases) {
      vi.mocked(api.submitAuth).mockRejectedValueOnce(new Error("Synthetic auth rejection"));
      const { onRefresh, unmount } = renderGate({ stage });
      const input = screen.getByLabelText(label);

      fireEvent.change(input, { target: { value: "synthetic-value" } });
      fireEvent.click(screen.getByRole("button", { name: "Continue" }));

      expect(await screen.findByRole("alert")).toHaveTextContent("Synthetic auth rejection");
      expect(input).toHaveValue("");
      expect(onRefresh).not.toHaveBeenCalled();

      unmount();
      vi.clearAllMocks();
    }
  });

  it("retains non-secret fields for correction after rejected submissions", async () => {
    for (const [stage, label] of nonSecretCases) {
      vi.mocked(api.submitAuth).mockRejectedValueOnce(new Error("Synthetic auth rejection"));
      const { onRefresh, unmount } = renderGate({ stage });
      const input = screen.getByLabelText(label);

      fireEvent.change(input, { target: { value: "synthetic-value" } });
      fireEvent.submit(input.closest("form")!);

      expect(await screen.findByRole("alert")).toHaveTextContent("Synthetic auth rejection");
      expect(input).toHaveValue("synthetic-value");
      expect(onRefresh).not.toHaveBeenCalled();

      unmount();
      vi.clearAllMocks();
    }
  });

  it("offers QR authentication only from the phone stage", async () => {
    const { onRefresh, unmount } = renderGate({ stage: "waiting_for_phone" });

    fireEvent.click(screen.getByRole("button", { name: /Sign in with QR code/ }));
    await waitFor(() => expect(api.requestQrAuth).toHaveBeenCalledTimes(1));
    expect(onRefresh).toHaveBeenCalledTimes(1);

    unmount();
    vi.clearAllMocks();
    for (const stage of nonPhoneStages) {
      const view = renderGate({ stage });
      expect(screen.queryByRole("button", { name: /Sign in with QR code/ })).not.toBeInTheDocument();
      view.unmount();
    }
  });

  it("renders a Telegram device-link QR image while waiting for another device", async () => {
    renderGate({ stage: "waiting_for_other_device", qrLink: "tg://login?token=synthetic-token" });

    expect(await screen.findByRole("img", { name: "Telegram device-link QR code" })).toBeInTheDocument();
    expect(api.requestQrAuth).not.toHaveBeenCalled();
  });
});
