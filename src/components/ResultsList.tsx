import { avatarSeed, resourceKey } from "../providers/identity";
import { Check, LoaderCircle, Pin, SearchX, ShieldAlert } from "lucide-react";
import type { ChatSummary, MessageSnapshot, SensitiveDataKind } from "../types";
import { Avatar, ContentIcon, contentLabel, formatCompactDate, plural } from "./common";

interface ResultsListProps {
  provider: "telegram" | "discord";
  messages: MessageSnapshot[];
  chats: ChatSummary[];
  selectedKeys: Set<string>;
  completedKeys: Set<string>;
  loading: boolean;
  refreshing: boolean;
  query: string;
  privacyScan: boolean;
  truncated: boolean;
  onToggle: (message: MessageSnapshot) => void;
  onToggleAll: () => void;
}

export const messageKey = (message: Pick<MessageSnapshot, "scope" | "messageId">) => resourceKey(message.scope, "content", message.messageId);

export function ResultsList({ provider, messages, chats, selectedKeys, completedKeys, loading, refreshing, query, privacyScan, truncated, onToggle, onToggleAll }: ResultsListProps) {
  const chatMap = new Map(chats.map((chat) => [resourceKey(chat.scope, "conversation", chat.id), chat]));
  const selectable = messages.filter(message => !completedKeys.has(messageKey(message)));
  const allSelected = selectable.length > 0 && selectable.every((message) => selectedKeys.has(messageKey(message)));

  return (
    <section className="results-section" aria-label="Search results">
      <div className="results-header">
        <label className="select-all">
          <input type="checkbox" checked={allSelected} onChange={onToggleAll} disabled={selectable.length === 0} />
          <span className="custom-checkbox">{allSelected && <Check size={12} />}</span>
          <span>{loading ? "Searching…" : plural(messages.length, "result")}</span>
        </label>
        <span className="results-note">
          {refreshing ? <><LoaderCircle className="spin" size={12} /> Syncing cleanup…</> : truncated ? "Result limit reached · refine the search" : privacyScan ? "Privacy findings · review each match" : "Newest first"}
        </span>
      </div>

      <div className="message-list" aria-busy={loading}>
        {loading && messages.length === 0 && (
          <div className="center-state"><LoaderCircle className="spin" size={24} /><p>{privacyScan ? "Scanning message history locally for sensitive information…" : provider === "discord" ? "Searching the local Discord archive…" : "Searching local and Telegram indexes…"}</p></div>
        )}
        {!loading && messages.length === 0 && (
          <div className="center-state">
            <SearchX size={31} strokeWidth={1.5} />
            <h2>{privacyScan ? "No sensitive information found" : query ? "No messages found" : "No messages in this scope"}</h2>
            <p>{privacyScan ? "Try another scope or date range. Automated detection can miss context and text inside images." : "Try another phrase, include pinned messages, or choose a different chat."}</p>
          </div>
        )}
        {messages.map((message) => {
          const key = messageKey(message);
          const checked = selectedKeys.has(key);
          const completed = completedKeys.has(key);
          const chat = chatMap.get(resourceKey(message.scope, "conversation", message.chatId));
          return (
            <article className={`message-row ${checked ? "is-selected" : ""} ${completed ? "is-complete" : ""}`} key={key}>
              <label className="message-check">
                <input type="checkbox" aria-label={`Select message from ${message.senderName}`} checked={checked} disabled={completed} onChange={() => onToggle(message)} />
                <span className="custom-checkbox">{checked && <Check size={12} />}</span>
              </label>
              <Avatar name={message.senderName} seed={avatarSeed(message.senderId)} size={35} />
              <button type="button" className="message-body" disabled={completed} onClick={() => onToggle(message)}>
                <span className="message-meta">
                  <strong>{message.senderName}</strong>
                  <span className="meta-dot">·</span>
                  <span>{chat?.title || "Unknown chat"}</span>
                  <span className="meta-dot">·</span>
                  <time dateTime={message.sentAt}>{formatCompactDate(message.sentAt)}</time>
                  {message.isPinned && <span className="pinned-label"><Pin size={11} /> pinned</span>}
                </span>
                <span className="message-preview">{message.preview}</span>
                <span className="message-tags">
                  <span className="content-tag"><ContentIcon kind={message.contentKind} />{contentLabel(message.contentKind)}</span>
                  {message.albumId && <span className="album-tag">Album</span>}
                  {message.privacyFindings.map((finding) => (
                    <span className="privacy-finding-tag" key={finding}><ShieldAlert size={10} />{privacyFindingLabel(finding)}</span>
                  ))}
                </span>
              </button>
              <ReachBadge provider={provider} reach={message.deletionReach} completed={completed} />
            </article>
          );
        })}
      </div>
    </section>
  );
}

function privacyFindingLabel(finding: SensitiveDataKind): string {
  const labels: Record<SensitiveDataKind, string> = {
    email_address: "Email",
    phone_number: "Phone",
    postal_address: "Address",
    precise_location: "Location",
    personal_identifier: "Personal identifier",
    identity_document: "Identity document",
    financial_account: "Financial account",
    crypto_wallet: "Crypto wallet",
    credential_or_secret: "Secret or credential",
    network_address: "Network address",
    contact_card: "Contact card"
  };
  return labels[finding];
}

function ReachBadge({ provider, reach, completed }: { provider: "telegram" | "discord"; reach: MessageSnapshot["deletionReach"]; completed: boolean }) {
  const copy = completed ? "Deleted / absent" : reach === "everyone" ? "Everyone" : reach === "self_only" ? "Only you" : "Protected";
  const title = completed ? "Discord confirmed deletion or reported that this exact archived message was already absent."
    : reach === "everyone"
    ? provider === "discord" ? "This exact owner-authored archive message is eligible for Discord deletion after matching-account verification." : "Telegram currently allows this message and its attached media to be deleted for all chat members. Externally saved copies are outside Telegram’s control."
    : reach === "self_only"
      ? "This message could only be removed from your own history. Retract will not silently do that."
      : provider === "discord" ? "This archived item is not eligible for Discord deletion." : "Telegram does not currently allow this account to delete the message.";
  return <span className={`reach-badge reach-${completed ? "complete" : reach}`} title={title}><span />{copy}</span>;
}
