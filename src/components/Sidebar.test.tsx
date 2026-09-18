import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { lifecycleChats } from "../test/lifecycle-wire";
import type { ChatSummary, DiscordNavigation } from "../types";
import { Sidebar } from "./Sidebar";

function discordChat(index: number, title: string, navigation: DiscordNavigation): ChatSummary {
  return { ...lifecycleChats[index], title, archived: true, capabilities: { ...lifecycleChats[index].capabilities, role: "member" }, discordNavigation: navigation };
}

const chats = [
  discordChat(0, "Bob", { category: "direct", groupId: null, groupLabel: null, detail: "Direct message" }),
  discordChat(1, "general", { category: "server", groupId: "server-1", groupLabel: "Pryzm Labs", detail: "Pryzm Labs" }),
  discordChat(2, "Unknown Discord chat", { category: "other", groupId: null, groupLabel: null, detail: "Unclassified chat" })
];

function renderDiscordSidebar(scope: Parameters<typeof Sidebar>[0]["scope"] = "all") {
  const onScopeChange = vi.fn();
  render(<Sidebar chats={chats} selectedChatId={null} scope={scope} chatQuery="" accountLabel="Ada Example" provider="discord"
    pendingRemovalChatIds={new Set()} onChatQueryChange={() => {}} onSelectChat={() => {}} onScopeChange={onScopeChange}
    onOpenSettings={() => {}} onOpenSources={() => {}} />);
  return { onScopeChange, navigation: within(screen.getByRole("complementary", { name: "Chat navigation" })) };
}

it("groups Discord conversations and replaces Telegram scopes with useful Discord filters", () => {
  const { onScopeChange, navigation } = renderDiscordSidebar();
  expect(navigation.getByRole("button", { name: "All chats 3" })).toBeVisible();
  expect(navigation.getByRole("button", { name: "Direct messages 1" })).toBeVisible();
  expect(navigation.getByRole("button", { name: "Servers 1" })).toBeVisible();
  expect(navigation.getByRole("button", { name: "Other 1" })).toBeVisible();
  expect(navigation.queryByRole("button", { name: /No reply sent/ })).not.toBeInTheDocument();
  expect(navigation.queryByRole("button", { name: /Admin access/ })).not.toBeInTheDocument();
  expect(navigation.queryByRole("button", { name: /^Archive/ })).not.toBeInTheDocument();

  expect(navigation.getByRole("heading", { name: "Direct messages 1" })).toBeVisible();
  const serverGroup = navigation.getByRole("button", { name: "Pryzm Labs 1" });
  expect(serverGroup).toBeVisible();
  expect(navigation.getByRole("button", { name: /general/ })).not.toBeVisible();
  fireEvent.click(serverGroup);
  expect(navigation.getByRole("button", { name: /general/ })).toBeVisible();
  expect(navigation.getByRole("heading", { name: "Other 1" })).toBeVisible();
  expect(navigation.queryByText("Member")).not.toBeInTheDocument();
  expect(navigation.getByText("Direct message")).toBeVisible();
  expect(navigation.getByText("Unclassified chat")).toBeVisible();

  fireEvent.click(navigation.getByRole("button", { name: "Servers 1" }));
  expect(onScopeChange).toHaveBeenCalledWith("servers");
});

it("filters Discord rows by navigation category while preserving group headings", () => {
  const { navigation } = renderDiscordSidebar("direct");
  expect(navigation.getByRole("button", { name: /Bob/ })).toBeVisible();
  expect(navigation.queryByRole("button", { name: /general/ })).not.toBeInTheDocument();
  expect(navigation.getByRole("heading", { name: "Direct messages 1" })).toBeVisible();
});
