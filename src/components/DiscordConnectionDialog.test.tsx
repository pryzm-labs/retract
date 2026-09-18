import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "@retract/api";
import { DiscordConnectionDialog } from "./DiscordConnectionDialog";
import { fixtureContext } from "../demo";
import { providerKey } from "../providers/identity";

const context = { ...fixtureContext, scope: { ...fixtureContext.scope, provider: providerKey("discord") } };

afterEach(() => vi.restoreAllMocks());

it("always offers one-way manual token entry alongside discovered browsers", async () => {
  vi.spyOn(api, "discordSession").mockResolvedValue({ state: "disconnected", accountId: null, username: null, displayName: null, remembered: false });
  vi.spyOn(api, "discordBrowsers").mockResolvedValue([{ id: "firefox", displayName: "Firefox", family: "firefox_bidi" }]);
  let finish!: () => void;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  const submit = vi.spyOn(api, "submitDiscordToken").mockImplementation(async () => { await pending; return { state: "ready", accountId: "123", username: "owner", displayName: "Owner", remembered: false }; });
  render(<DiscordConnectionDialog context={context} onClose={() => {}} onReady={() => {}} onError={() => {}} />);
  expect(await screen.findByRole("button", { name: "Continue with Firefox" })).toBeDisabled();
  expect(screen.getByLabelText("Discord token")).toHaveAttribute("type", "password");
  fireEvent.click(screen.getByText(/I understand the Discord account risk/));
  fireEvent.change(screen.getByLabelText("Discord token"), { target: { value: "a-valid-looking-user-token-value" } });
  fireEvent.click(screen.getByRole("button", { name: "Verify token" }));
  expect(screen.getByLabelText("Discord token")).toHaveValue("");
  expect(submit).toHaveBeenCalledWith("a-valid-looking-user-token-value", false, true, context);
  finish();
  await waitFor(() => expect(screen.getByText("Owner")).toBeInTheDocument());
});

it("explains browser-neutral manual capture without asking for cookies", async () => {
  vi.spyOn(api, "discordSession").mockResolvedValue({ state: "disconnected", accountId: null, username: null, displayName: null, remembered: false });
  vi.spyOn(api, "discordBrowsers").mockResolvedValue([]);
  render(<DiscordConnectionDialog context={context} onClose={() => {}} onReady={() => {}} onError={() => {}} />);
  fireEvent.click(await screen.findByText("How to copy a token manually"));
  expect(screen.getByText(/any browser/)).toBeInTheDocument();
  expect(screen.getByText(/Never paste cookies/)).toBeInTheDocument();
});
