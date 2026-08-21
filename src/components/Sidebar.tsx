import {
  Archive,
  CircleOff,
  ChevronsUpDown,
  Inbox,
  LoaderCircle,
  MessageCircleOff,
  Search,
  Settings2,
  ShieldCheck
} from "lucide-react";
import type { ChatSummary } from "../types";
import { BrandLogo } from "./BrandLogo";
import { Avatar, RoleIcon, roleLabel } from "./common";

export type ChatScope = "all" | "unanswered" | "empty" | "admin" | "archive";

interface SidebarProps {
  chats: ChatSummary[];
  selectedChatId: number | null;
  scope: ChatScope;
  chatQuery: string;
  accountLabel: string;
  pendingRemovalChatIds: Set<number>;
  onChatQueryChange: (value: string) => void;
  onSelectChat: (id: number | null) => void;
  onScopeChange: (scope: ChatScope) => void;
  onOpenSettings: () => void;
}

export function Sidebar({
  chats,
  selectedChatId,
  scope,
  chatQuery,
  accountLabel,
  pendingRemovalChatIds,
  onChatQueryChange,
  onSelectChat,
  onScopeChange,
  onOpenSettings
}: SidebarProps) {
  const lowered = chatQuery.trim().toLowerCase();
  const visibleChats = chats.filter((chat) => {
    if (scope === "unanswered" && chat.conversationState !== "never_replied") return false;
    if (scope === "empty" && chat.conversationState !== "empty") return false;
    if (scope === "admin" && !["owner", "admin_with_delete", "admin_limited"].includes(chat.capabilities.role)) return false;
    if (scope === "archive" && !chat.archived) return false;
    return !lowered || chat.title.toLowerCase().includes(lowered);
  });

  return (
    <aside className="sidebar" aria-label="Chat navigation">
      <div className="brand-row">
        <BrandLogo />
        <span className="brand-name">Retract</span>
        <span className="runtime-pill">TELEGRAM</span>
      </div>

      <button className="account-switcher" type="button" aria-label="Current account">
        <Avatar name={accountLabel} seed={3} size={30} />
        <span className="account-copy">
          <strong>{accountLabel}</strong>
          <small>Telegram account</small>
        </span>
        <ChevronsUpDown size={15} aria-hidden="true" />
      </button>

      <nav className="scope-nav" aria-label="Chat scopes">
        <ScopeButton icon={<Inbox />} label="All chats" count={chats.length} active={scope === "all" && selectedChatId === null} onClick={() => { onScopeChange("all"); onSelectChat(null); }} />
        <ScopeButton
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
      </nav>

      <div className="sidebar-section-heading">
        <span>{scopeHeading(scope)}</span>
        <button type="button" className="icon-button tiny" aria-label="Open connection settings" onClick={onOpenSettings}><Settings2 size={14} /></button>
      </div>

      <div className={`sidebar-context ${scope === "unanswered" || scope === "empty" ? "scope-guidance" : "capability-key"}`}>
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
      </div>

      <label className="chat-search">
        <Search size={14} aria-hidden="true" />
        <input value={chatQuery} onChange={(event) => onChatQueryChange(event.target.value)} placeholder="Find a chat" aria-label="Find a chat" />
      </label>

      <div className="chat-list" aria-label="Chats">
        {visibleChats.map((chat) => {
          const removalPending = pendingRemovalChatIds.has(chat.id);
          return (
            <button
              type="button"
              className={`chat-row ${selectedChatId === chat.id ? "is-selected" : ""} ${removalPending ? "is-pending-removal" : ""}`}
              key={chat.id}
              onClick={() => onSelectChat(chat.id)}
              disabled={removalPending}
            >
              <Avatar name={chat.title} seed={chat.avatarSeed} size={34} />
              <span className="chat-copy">
                <span className="chat-title-line">
                  <strong>{chat.title}</strong>
                  {chat.capabilities.canClearForEveryone && !removalPending && <span className="clearable-dot" role="img" aria-label="Full-history revoke available" title="This chat can be cleared for everyone" />}
                </span>
                {removalPending ? (
                  <span className="role-line pending-removal-label"><LoaderCircle className="spin" size={12} />Removing…</span>
                ) : (
                  <span className={`role-line role-${chat.capabilities.role}`}>
                    {conversationLabel(chat) && <span className={`conversation-state state-${chat.conversationState}`}>{conversationLabel(chat)}</span>}
                    {conversationLabel(chat) && <span aria-hidden="true">·</span>}
                    <RoleIcon role={chat.capabilities.role} />
                    {roleLabel(chat.capabilities.role)}
                    {chat.memberCount ? ` · ${chat.memberCount.toLocaleString()}` : ""}
                  </span>
                )}
              </span>
            </button>
          );
        })}
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
  return "CHATS";
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
