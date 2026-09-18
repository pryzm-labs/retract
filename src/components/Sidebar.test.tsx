import { StrictMode } from "react";
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
  discordChat(2, "Archived conversation …12345678", { category: "other", groupId: null, groupLabel: null, detail: "Name unavailable in Discord export" })
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
  const chatList = within(navigation.getByLabelText("Chats"));
  const scopes = within(navigation.getByRole("navigation", { name: "Chat scopes" }));
  expect(scopes.getByRole("button", { name: "All chats 3" })).toBeVisible();
  expect(scopes.getByRole("button", { name: "Direct messages 1" })).toBeVisible();
  expect(scopes.getByRole("button", { name: "Servers 1" })).toBeVisible();
  expect(scopes.getByRole("button", { name: "Unclassified 1" })).toBeVisible();
  expect(navigation.queryByRole("button", { name: /No reply sent/ })).not.toBeInTheDocument();
  expect(navigation.queryByRole("button", { name: /Admin access/ })).not.toBeInTheDocument();
  expect(navigation.queryByRole("button", { name: /^Archive/ })).not.toBeInTheDocument();

  const directGroup = chatList.getByRole("button", { name: "Direct messages 1" });
  expect(directGroup).toHaveAttribute("aria-expanded", "true");
  fireEvent.click(directGroup);
  expect(directGroup).toHaveAttribute("aria-expanded", "false");
  expect(navigation.queryByRole("button", { name: /Bob/ })).not.toBeInTheDocument();
  fireEvent.click(directGroup);
  const serverGroup = chatList.getByRole("button", { name: "Pryzm Labs 1" });
  expect(serverGroup).toBeVisible();
  expect(navigation.queryByRole("button", { name: /general/ })).not.toBeInTheDocument();
  fireEvent.click(serverGroup);
  expect(navigation.getByRole("button", { name: /general/ })).toBeVisible();
  const unclassifiedGroup = chatList.getByRole("button", { name: "Unclassified 1" });
  expect(unclassifiedGroup).toHaveAttribute("aria-expanded", "false");
  expect(navigation.queryByRole("button", { name: /Archived conversation/ })).not.toBeInTheDocument();
  fireEvent.click(unclassifiedGroup);
  expect(unclassifiedGroup).toHaveAttribute("aria-expanded", "true");
  expect(navigation.getByRole("button", { name: /Archived conversation/ })).toBeVisible();
  expect(navigation.queryByText("Member")).not.toBeInTheDocument();
  expect(navigation.getByText("Direct message")).toBeVisible();
  expect(navigation.getByText("Name unavailable in Discord export")).toBeVisible();

  fireEvent.click(scopes.getByRole("button", { name: "Servers 1" }));
  expect(onScopeChange).toHaveBeenCalledWith("servers");
});

it("expands and collapses server groups safely under StrictMode", () => {
  render(<StrictMode><Sidebar chats={chats} selectedChatId={null} scope="all" chatQuery="" accountLabel="Ada Example" provider="discord"
    pendingRemovalChatIds={new Set()} onChatQueryChange={() => {}} onSelectChat={() => {}} onScopeChange={() => {}}
    onOpenSettings={() => {}} onOpenSources={() => {}} /></StrictMode>);
  const navigation = within(screen.getByRole("complementary", { name: "Chat navigation" }));
  const serverGroup = navigation.getByRole("button", { name: "Pryzm Labs 1" });

  fireEvent.click(serverGroup);
  expect(serverGroup).toHaveAttribute("aria-expanded", "true");
  expect(navigation.getByRole("button", { name: /general/ })).toBeVisible();
  fireEvent.click(serverGroup);
  expect(serverGroup).toHaveAttribute("aria-expanded", "false");
  expect(navigation.queryByRole("button", { name: /general/ })).not.toBeInTheDocument();
});

it("filters Discord rows by navigation category while preserving group headings", () => {
  const { navigation } = renderDiscordSidebar("direct");
  const chatList = within(navigation.getByLabelText("Chats"));
  expect(navigation.getByRole("button", { name: /Bob/ })).toBeVisible();
  expect(navigation.queryByRole("button", { name: /general/ })).not.toBeInTheDocument();
  expect(chatList.getByRole("button", { name: "Direct messages 1" })).toBeVisible();
});
