import { AlertOctagon, Check, Eraser, LockKeyhole, LogOut, ShieldAlert, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { PlanView } from "../types";
import { plural } from "./common";

interface ConfirmDialogProps {
  plan: PlanView;
  busy: boolean;
  onClose: () => void;
  onConfirm: (acknowledged: boolean, typedTitle: string | null) => void;
}

export function ConfirmDialog({ plan, busy, onClose, onConfirm }: ConfirmDialogProps) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [typedTitle, setTypedTitle] = useState("");
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleRequired = plan.confirmationTier === "high" || plan.confirmationTier === "critical";
  const titleMatches = !titleRequired || typedTitle === plan.chatTitle;
  const critical = plan.confirmationTier === "critical";
  const leavesChat = plan.operation === "leave_chat"
    || plan.operation === "delete_all_messages_and_leave"
    || plan.operation === "clear_history_and_leave";
  const canConfirm = acknowledged && titleMatches && !busy;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  const heading = plan.operation === "delete_group"
    ? `Delete “${plan.chatTitle}” permanently?`
    : plan.operation === "clear_history_and_leave"
      ? `Clear all history and leave “${plan.chatTitle}”?`
    : plan.operation === "delete_all_messages_and_leave"
      ? `Delete all possible history and leave “${plan.chatTitle}”?`
    : plan.operation === "leave_chat"
      ? `Leave “${plan.chatTitle}” and remove it?`
    : plan.operation === "clear_history"
      ? `Revoke history and remove “${plan.chatTitle}”?`
    : plan.operation === "remove_chat_for_self"
      ? `Remove “${plan.chatTitle}” from your chat list?`
      : plan.operation === "delete_my_messages"
        ? `Delete all your messages from “${plan.chatTitle}”?`
      : plan.operation === "delete_by_sender"
        ? `Delete every message by ${plan.targetSenderName}?`
      : `Delete ${plural(plan.summary.deleteForEveryone, "message")} for everyone?`;

  return (
    <dialog
      ref={dialogRef}
      className={`confirm-dialog ${critical ? "is-critical" : ""}`}
      onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}
    >
      <div className="dialog-icon">{critical ? <AlertOctagon /> : <ShieldAlert />}</div>
      <button type="button" className="dialog-close" onClick={onClose} disabled={busy} aria-label="Close confirmation"><X size={18} /></button>
      <p className="eyebrow">{critical ? "CRITICAL ACTION" : "FINAL REVIEW"}</p>
      <h2>{heading}</h2>
      <p className="dialog-lead">
        {plan.operation === "delete_group"
          ? "Telegram will remove every member and message, release public usernames, and dissolve this group. Retract cannot reverse it."
          : plan.operation === "clear_history_and_leave"
            ? "Retract will ask Telegram to clear the complete history for everyone before leaving the group or channel and removing its remaining local entry. This is the broadest history cleanup Telegram currently reports as available; externally saved copies remain outside Telegram’s control."
          : plan.operation === "delete_all_messages_and_leave"
            ? `Retract froze ${plural(plan.summary.selected, "message")} from every participant in this chat. It will recheck and delete every eligible message for everyone, including attached media and captions, then leave and remove the remaining local entry. Rejected or protected messages will be reported without a self-only fallback.`
          : plan.operation === "leave_chat"
            ? plan.summary.deleteForEveryone > 0
              ? `Retract will first recheck and attempt to delete ${plural(plan.summary.deleteForEveryone, "message")} sent by your account for everyone, including attached media and captions. Messages Telegram no longer permits will be skipped or reported as failed. Retract will then leave the chat and remove its remaining local history; other participants’ messages and saved copies remain.`
              : "Retract found no messages sent by your account that Telegram currently permits it to revoke for everyone. It will leave the chat and remove the remaining history from your chat list; other participants’ copies remain."
          : plan.operation === "clear_history"
            ? "Every message Telegram permits will disappear for all chat members, and the conversation will be removed from your chat list. A future DM can recreate the conversation; group membership remains unless the group itself is permanently deleted."
          : plan.operation === "remove_chat_for_self"
            ? "Telegram will delete this chat history only for your account and remove the conversation from your chat list. The other participant or group members keep their copies. A future message can recreate the conversation."
            : plan.operation === "delete_my_messages"
              ? "Only messages sent by your account in this group will be removed for everyone. Your membership and every other participant’s messages remain. Attached media and captions are removed with their message; externally saved copies are outside Telegram’s control."
            : plan.operation === "delete_by_sender"
              ? `Telegram will remove every message sent by ${plan.targetSenderName} in “${plan.chatTitle}”. This can affect far more messages than the current search results.`
            : "Only the messages in this frozen plan that still pass a live capability check will be removed. Attached media and captions are removed with their message; externally saved copies are outside Telegram’s control."}
      </p>

      {(plan.operation === "selected_messages" || plan.operation === "delete_my_messages" || plan.operation === "delete_all_messages_and_leave" || plan.operation === "leave_chat") && (
        <div className="dialog-summary">
          <div><strong>{plan.summary.deleteForEveryone}</strong><span>delete for everyone</span></div>
          <div><strong>{plan.summary.selfOnly}</strong><span>self-only skipped</span></div>
          <div><strong>{plan.summary.cannotDelete}</strong><span>protected skipped</span></div>
        </div>
      )}

      <div className="plan-binding">
        <LockKeyhole size={14} /> Plan <code>{plan.fingerprint.slice(0, 12)}</code> is frozen to {plan.operation === "selected_messages" ? "the reviewed IDs" : plan.operation === "delete_my_messages" ? "your message IDs in this chat" : plan.operation === "delete_all_messages_and_leave" ? "every enumerated message ID and this chat" : plan.operation === "leave_chat" ? "your outgoing message IDs and this chat" : plan.operation === "clear_history_and_leave" ? "whole-history cleanup and this immutable chat" : plan.operation === "delete_by_sender" ? "this chat and sender" : "this immutable chat"}.
      </div>

      <p className="system-auth-note">macOS will show the exact frozen target and verify the device owner after you press the final button.</p>

      {titleRequired && (
        <label className="typed-confirmation">
          Type <strong>{plan.chatTitle}</strong> to continue
          <input
            value={typedTitle}
            onChange={(event) => setTypedTitle(event.target.value)}
            autoComplete="off"
            spellCheck={false}
            placeholder={plan.chatTitle || "Chat title"}
            autoFocus
          />
        </label>
      )}

      <label className="irreversible-check">
        <input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
        <span className="custom-checkbox">{acknowledged && <Check size={12} />}</span>
        <span>{plan.operation === "clear_history_and_leave" ? "I understand this attempts complete history revocation before removing my membership and local entry." : plan.operation === "delete_all_messages_and_leave" ? "I understand Retract will attempt to revoke every frozen eligible message before leaving and removing my local entry." : plan.operation === "leave_chat" ? "I understand Retract will try to revoke my frozen outgoing messages, then remove my membership and remaining copy." : plan.operation === "remove_chat_for_self" ? "I understand this deletes only my history and chat-list entry, not anyone else’s copy." : "I understand that accepted Telegram deletions cannot be undone."}</span>
      </label>

      <div className="dialog-actions">
        <button type="button" className="cancel-button" onClick={onClose} disabled={busy}>Cancel</button>
        <button type="button" className={`confirm-button ${critical ? "critical" : ""}`} disabled={!canConfirm} onClick={() => onConfirm(acknowledged, typedTitle || null)}>
          {leavesChat ? <LogOut size={16} /> : <Eraser size={16} />}
          {busy ? "Starting safely…" : critical ? "Delete group permanently" : plan.operation === "clear_history_and_leave" ? "Clear all history & leave" : plan.operation === "delete_all_messages_and_leave" ? "Delete all possible & leave" : plan.operation === "leave_chat" ? "Revoke my messages & leave" : plan.operation === "remove_chat_for_self" ? "Remove chat for me" : plan.operation === "delete_my_messages" ? "Delete all my messages" : plan.operation === "delete_by_sender" ? "Delete sender’s messages" : plan.operation === "clear_history" ? "Revoke & remove chat" : "Delete for everyone"}
        </button>
      </div>
    </dialog>
  );
}
