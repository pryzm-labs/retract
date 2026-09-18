import { refKey } from "../providers/identity";
import { useState } from "react";
import {
  Archive,
  CircleHelp,
  CircleOff,
  ChevronRight,
  ChevronsUpDown,
  Inbox,
  LoaderCircle,
  MessagesSquare,
  MessageCircleOff,
  Search,
  Server,
  Settings2,
  ShieldCheck
} from "lucide-react";
import type { ChatSummary } from "../types";
import { BrandLogo } from "./BrandLogo";
import { Avatar, RoleIcon, roleLabel } from "./common";

export type ChatScope = "all" | "unanswered" | "empty" | "admin" | "archive" | "direct" | "servers" | "other";

interface SidebarProps {
  chats: ChatSummary[];
  selectedChatId: string | null;
  scope: ChatScope;
  chatQuery: string;
  accountLabel: string;
  provider: "telegram" | "discord";
  pendingRemovalChatIds: Set<string>;
  onChatQueryChange: (value: string) => void;
  onSelectChat: (id: string | null) => void;
  onScopeChange: (scope: ChatScope) => void;
  onOpenSettings: () => void;
  onOpenSources: () => void;
}

export function Sidebar({
  chats,
  selectedChatId,
  scope,
  chatQuery,
  accountLabel,
  provider,
  pendingRemovalChatIds,
  onChatQueryChange,
  onSelectChat,
  onScopeChange,
  onOpenSettings,
  onOpenSources
}: SidebarProps) {
  const lowered = chatQuery.trim().toLowerCase();
  const visibleChats = chats.filter((chat) => {
    if (scope === "unanswered" && chat.conversationState !== "never_replied") return false;
    if (scope === "empty" && chat.conversationState !== "empty") return false;
    if (scope === "admin" && !["owner", "admin_with_delete", "admin_limited"].includes(chat.capabilities.role)) return false;
    if (scope === "archive" && !chat.archived) return false;
    if (scope === "direct" && chat.discordNavigation?.category !== "direct") return false;
    if (scope === "servers" && chat.discordNavigation?.category !== "server") return false;
    if (scope === "other" && chat.discordNavigation?.category !== "other") return false;
    return !lowered || [chat.title, chat.discordNavigation?.groupLabel, chat.discordNavigation?.detail]
      .some(value => value?.toLowerCase().includes(lowered));
  });
  const directCount = chats.filter(chat => chat.discordNavigation?.category === "direct").length;
  const serverCount = new Set(chats.filter(chat => chat.discordNavigation?.category === "server").map(chat => chat.discordNavigation?.groupId ?? chat.discordNavigation?.groupLabel ?? refKey(chat.ref))).size;
  const otherCount = chats.filter(chat => chat.discordNavigation?.category === "other").length;

  return (
    <aside className="sidebar" aria-label="Chat navigation">
      <div className="brand-row">
        <BrandLogo />
        <span className="brand-name">Retract</span>
        <span className="runtime-pill">{provider.toUpperCase()}</span>
      </div>

      <button className="account-switcher" type="button" aria-label="Switch data source" onClick={onOpenSources}>
        <Avatar name={accountLabel} seed={3} size={30} />
        <span className="account-copy">
          <strong>{accountLabel}</strong>
          <small>{provider === "discord" ? "Discord archive" : "Telegram account"}</small>
        </span>
        <ChevronsUpDown size={15} aria-hidden="true" />
      </button>

      <nav className="scope-nav" aria-label="Chat scopes">
        <ScopeButton icon={<Inbox />} label="All chats" count={chats.length} active={scope === "all" && selectedChatId === null} onClick={() => { onScopeChange("all"); onSelectChat(null); }} />
        {provider === "discord" ? <>
          <ScopeButton icon={<MessagesSquare />} label="Direct messages" count={directCount} active={scope === "direct" && selectedChatId === null} onClick={() => { onScopeChange("direct"); onSelectChat(null); }} />
          <ScopeButton icon={<Server />} label="Servers" count={serverCount} active={scope === "servers" && selectedChatId === null} onClick={() => { onScopeChange("servers"); onSelectChat(null); }} />
          <ScopeButton icon={<CircleHelp />} label="Other" count={otherCount} active={scope === "other" && selectedChatId === null} onClick={() => { onScopeChange("other"); onSelectChat(null); }} />
        </> : <><ScopeButton
          icon={<MessageCircleOff />}
          label="No reply sent"
          count={chats.filter((chat) => chat.conversationState === "never_replied").length}
          active={scope === "unanswered" && selectedChatId === null}
          onClick={() => { onScopeChange("unanswered"); onSelectChat(null); }}
        />
        <ScopeButton
          icon={<CircleOff />}
          label="Empty"
          count={chats.filter((chat) => chat.conversationState === "empty").length}
          active={scope === "empty" && selectedChatId === null}
          onClick={() => { onScopeChange("empty"); onSelectChat(null); }}
        />
        <ScopeButton
          icon={<ShieldCheck />}
          label="Admin access"
          count={chats.filter((chat) => chat.capabilities.role !== "member").length}
          active={scope === "admin" && selectedChatId === null}
          onClick={() => { onScopeChange("admin"); onSelectChat(null); }}
        />
        <ScopeButton icon={<Archive />} label="Archive" count={chats.filter((chat) => chat.archived).length} active={scope === "archive" && selectedChatId === null} onClick={() => { onScopeChange("archive"); onSelectChat(null); }} />
        </>}
      </nav>

      <div className="sidebar-section-heading">
        <span>{scopeHeading(scope)}</span>
        <button type="button" className="icon-button tiny" aria-label="Open connection settings" onClick={onOpenSettings}><Settings2 size={14} /></button>
      </div>

      {provider !== "discord" && <div className={`sidebar-context ${scope === "unanswered" || scope === "empty" ? "scope-guidance" : "capability-key"}`}>
        {scope === "unanswered" ? (
          <>
            <span>No messages sent by this account were found.</span>
            <span>Ambiguous admin and secret-chat histories are excluded.</span>
          </>
        ) : scope === "empty" ? (
          <>
            <span>No history was found for this account.</span>
            <span>Review each conversation before removing it.</span>
          </>
        ) : (
          <><span className="clearable-dot" aria-hidden="true" /><span>Full-history revoke available</span></>
        )}
      </div>}

      <label className="chat-search">
        <Search size={14} aria-hidden="true" />
        <input value={chatQuery} onChange={(event) => onChatQueryChange(event.target.value)} placeholder="Find a chat" aria-label="Find a chat" />
      </label>

      <div className="chat-list" aria-label="Chats">
        {provider === "discord"
          ? <DiscordChatGroups chats={visibleChats} selectedChatId={selectedChatId} pendingRemovalChatIds={pendingRemovalChatIds} onSelectChat={onSelectChat} expandServers={Boolean(lowered)} />
          : visibleChats.map(chat => <ChatRow key={refKey(chat.ref)} chat={chat} selectedChatId={selectedChatId} removalPending={pendingRemovalChatIds.has(refKey(chat.ref))} onSelectChat={onSelectChat} />)}
        {visibleChats.length === 0 && <p className="empty-sidebar">No matching chats</p>}
      </div>
    </aside>
  );
}

function scopeHeading(scope: ChatScope): string {
  if (scope === "unanswered") return "NO REPLY SENT";
  if (scope === "empty") return "EMPTY CHATS";
  if (scope === "admin") return "MANAGED CHATS";
  if (scope === "archive") return "ARCHIVED";
  if (scope === "direct") return "DIRECT MESSAGES";
  if (scope === "servers") return "SERVERS";
  if (scope === "other") return "OTHER CHATS";
  return "CHATS";
}

function DiscordChatGroups({ chats, selectedChatId, pendingRemovalChatIds, onSelectChat, expandServers }: Pick<SidebarProps, "selectedChatId" | "pendingRemovalChatIds" | "onSelectChat"> & { chats: ChatSummary[]; expandServers: boolean }) {
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const direct = chats.filter(chat => chat.discordNavigation?.category === "direct");
  const other = chats.filter(chat => chat.discordNavigation?.category === "other" || !chat.discordNavigation);
  const servers = new Map<string, { label: string; chats: ChatSummary[] }>();
  for (const chat of chats.filter(item => item.discordNavigation?.category === "server")) {
    const key = chat.discordNavigation?.groupId ?? refKey(chat.ref);
    const label = chat.discordNavigation?.groupLabel ?? "Unknown server";
    const group = servers.get(key) ?? { label, chats: [] };
    group.chats.push(chat);
    servers.set(key, group);
  }
  const groups = [
    ...(direct.length ? [{ key: "direct", label: "Direct messages", chats: direct, collapsible: false }] : []),
    ...[...servers].sort(([, a], [, b]) => a.label.localeCompare(b.label)).map(([key, group]) => ({ key: `server:${key}`, label: group.label, chats: group.chats, collapsible: true })),
    ...(other.length ? [{ key: "other", label: "Other", chats: other, collapsible: false }] : [])
  ];
  const rows = (group: typeof groups[number]) => group.chats.map(chat => <ChatRow key={refKey(chat.ref)} chat={chat} selectedChatId={selectedChatId} removalPending={pendingRemovalChatIds.has(refKey(chat.ref))} onSelectChat={onSelectChat} discord />);
  return <>{groups.map(group => {
    const serverOpen = expandServers || expanded.has(group.key) || group.chats.some(chat => refKey(chat.ref) === selectedChatId);
    return group.collapsible
    ? <details className="discord-chat-group discord-server-group" key={group.key} open={serverOpen} onToggle={event => {
      if (expandServers || group.chats.some(chat => refKey(chat.ref) === selectedChatId)) return;
      setExpanded(current => { const next = new Set(current); if (event.currentTarget.open) next.add(group.key); else next.delete(group.key); return next; });
    }}>
      <summary role="button" aria-expanded={serverOpen} className="discord-chat-group-heading" aria-label={`${group.label} ${group.chats.length}`}><ChevronRight className="discord-group-chevron" size={13} /><span>{group.label}</span><span>{group.chats.length.toLocaleString()}</span></summary>
      {rows(group)}
    </details>
    : <section className="discord-chat-group" key={group.key}>
      <h3 className="discord-chat-group-heading" aria-label={`${group.label} ${group.chats.length}`}><span>{group.label}</span><span>{group.chats.length.toLocaleString()}</span></h3>
      {rows(group)}
    </section>;
  })}</>;
}

function ChatRow({ chat, selectedChatId, removalPending, onSelectChat, discord = false }: { chat: ChatSummary; selectedChatId: string | null; removalPending: boolean; onSelectChat: (id: string | null) => void; discord?: boolean }) {
  return <button type="button" className={`chat-row ${selectedChatId === refKey(chat.ref) ? "is-selected" : ""} ${removalPending ? "is-pending-removal" : ""}`}
    onClick={() => onSelectChat(refKey(chat.ref))} disabled={removalPending}>
    <Avatar name={chat.title} seed={chat.avatarSeed} size={34} />
    <span className="chat-copy">
      <span className="chat-title-line"><strong>{chat.title}</strong>
        {chat.capabilities.canClearForEveryone && !removalPending && <span className="clearable-dot" role="img" aria-label="Full-history revoke available" title="This chat can be cleared for everyone" />}
      </span>
      {removalPending ? <span className="role-line pending-removal-label"><LoaderCircle className="spin" size={12} />Removing…</span>
        : discord ? <span className="role-line discord-context-line">{chat.discordNavigation?.detail ?? "Discord chat"}</span>
          : <span className={`role-line role-${chat.capabilities.role}`}>
            {conversationLabel(chat) && <span className={`conversation-state state-${chat.conversationState}`}>{conversationLabel(chat)}</span>}
            {conversationLabel(chat) && <span aria-hidden="true">·</span>}
            <RoleIcon role={chat.capabilities.role} />{roleLabel(chat.capabilities.role)}
            {chat.memberCount ? ` · ${chat.memberCount.toLocaleString()}` : ""}
          </span>}
    </span>
  </button>;
}

function conversationLabel(chat: ChatSummary): string | null {
  if (chat.conversationState === "never_replied") return "No reply sent";
  if (chat.conversationState === "empty") return "Empty";
  if (chat.conversationState === "awaiting_reply") return "Waiting on you";
  return null;
}

function ScopeButton({ icon, label, count, active, onClick }: { icon: React.ReactElement<{ size?: number }>; label: string; count: number; active: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      className={`scope-button ${active ? "is-active" : ""}`}
      aria-label={`${label} ${count}`}
      onClick={onClick}
    >
      {icon}
      <span>{label}</span>
      <span className="scope-count">{count}</span>
    </button>
  );
}
