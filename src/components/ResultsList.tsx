import { Check, LoaderCircle, Pin, SearchX, ShieldAlert } from "lucide-react";
import type { ChatSummary, MessageSnapshot, SensitiveDataKind } from "../types";
import { Avatar, ContentIcon, contentLabel, formatCompactDate, plural } from "./common";

interface ResultsListProps {
  messages: MessageSnapshot[];
  chats: ChatSummary[];
  selectedKeys: Set<string>;
  loading: boolean;
  refreshing: boolean;
  query: string;
  privacyScan: boolean;
  truncated: boolean;
  onToggle: (message: MessageSnapshot) => void;
  onToggleAll: () => void;
}

export const messageKey = (message: Pick<MessageSnapshot, "chatId" | "messageId">) => `${message.chatId}:${message.messageId}`;

export function ResultsList({ messages, chats, selectedKeys, loading, refreshing, query, privacyScan, truncated, onToggle, onToggleAll }: ResultsListProps) {
  const chatMap = new Map(chats.map((chat) => [chat.id, chat]));
  const allSelected = messages.length > 0 && messages.every((message) => selectedKeys.has(messageKey(message)));

  return (
    <section className="results-section" aria-label="Search results">
      <div className="results-header">
        <label className="select-all">
          <input type="checkbox" checked={allSelected} onChange={onToggleAll} disabled={messages.length === 0} />
          <span className="custom-checkbox">{allSelected && <Check size={12} />}</span>
          <span>{loading ? "Searching…" : plural(messages.length, "result")}</span>
        </label>
        <span className="results-note">
          {refreshing ? <><LoaderCircle className="spin" size={12} /> Syncing cleanup…</> : truncated ? "Result limit reached · refine the search" : privacyScan ? "Privacy findings · review each match" : "Newest first"}
        </span>
      </div>

      <div className="message-list" aria-busy={loading}>
        {loading && messages.length === 0 && (
          <div className="center-state"><LoaderCircle className="spin" size={24} /><p>{privacyScan ? "Scanning message history locally for sensitive information…" : "Searching local and Telegram indexes…"}</p></div>
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
          const chat = chatMap.get(message.chatId);
          return (
            <article className={`message-row ${checked ? "is-selected" : ""}`} key={key}>
              <label className="message-check" aria-label={`Select message from ${message.senderName}`}>
                <input type="checkbox" checked={checked} onChange={() => onToggle(message)} />
                <span className="custom-checkbox">{checked && <Check size={12} />}</span>
              </label>
              <Avatar name={message.senderName} seed={Math.abs(message.senderId % 19)} size={35} />
              <button type="button" className="message-body" onClick={() => onToggle(message)}>
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
              <ReachBadge reach={message.deletionReach} />
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

function ReachBadge({ reach }: { reach: MessageSnapshot["deletionReach"] }) {
  const copy = reach === "everyone" ? "Everyone" : reach === "self_only" ? "Only you" : "Protected";
  const title = reach === "everyone"
    ? "Telegram currently allows this message and its attached media to be deleted for all chat members. Externally saved copies are outside Telegram’s control."
    : reach === "self_only"
      ? "This message could only be removed from your own history. Retract will not silently do that."
      : "Telegram does not currently allow this account to delete the message.";
  return <span className={`reach-badge reach-${reach}`} title={title}><span />{copy}</span>;
}
