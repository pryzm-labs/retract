import { AlertOctagon, Check, Eraser, LockKeyhole, LogOut, ShieldAlert, X } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { PlanView, PlanOperation } from "../types";
import { plural } from "./common";
import { ActionEffects } from "./ActionEffects";

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
  const titleId = useId();
  // Adapter operation names select familiar copy only when its stated primary
  // effect is present. They never authorize a target or replace the effect list.
  const displayEffects: Record<PlanOperation, string> = {
    selected_messages: "removed_for_all_participants", delete_my_messages: "removed_for_all_participants",
    clear_history: "removed_for_all_participants", clear_history_and_leave: "membership_removed",
    delete_all_messages_and_leave: "membership_removed", leave_chat: "membership_removed",
    remove_chat_for_self: "removed_for_current_account_only", delete_by_sender: "removed_for_all_participants",
    delete_group: "container_destroyed"
  };
  const operation = plan.steps.some(step => step.descriptor.effect === displayEffects[plan.operation]) ? plan.operation : null;
  const titleRequired = plan.confirmation.exactText !== null;
  const titleMatches = !titleRequired || typedTitle === plan.confirmation.exactText;
  const critical = plan.confirmation.tier === "critical";
  const destroysChat = plan.steps.some(step => step.descriptor.effect === "container_destroyed");
  const leavesChat = plan.steps.some(step => step.descriptor.effect === "membership_removed");
  const available = plan.steps.length > 0 && plan.steps.every(step => step.descriptor.availability === "executable" || step.descriptor.availability === "live_preflight_required");
  const canConfirm = available && (!plan.confirmation.acknowledgementRequired || acknowledged) && titleMatches && !busy;

  useEffect(() => {
    const previous = document.activeElement;
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    dialog?.querySelector<HTMLElement>(".typed-confirmation input, .dialog-close")?.focus();
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, []);

  const heading = operation === null ? "Review the frozen effects?" : destroysChat
    ? `Delete “${plan.chatTitle}” permanently?`
    : operation === "clear_history_and_leave"
      ? `Clear all history and leave “${plan.chatTitle}”?`
    : operation === "delete_all_messages_and_leave"
      ? `Delete all possible history and leave “${plan.chatTitle}”?`
    : operation === "leave_chat"
      ? `Leave “${plan.chatTitle}” and remove it?`
    : operation === "clear_history"
      ? `Revoke history and remove “${plan.chatTitle}”?`
    : operation === "remove_chat_for_self"
      ? `Remove “${plan.chatTitle}” from your chat list?`
      : operation === "delete_my_messages"
        ? `Delete all your messages from “${plan.chatTitle}”?`
      : operation === "delete_by_sender"
        ? `Delete every message by ${plan.targetSenderName}?`
      : `Delete ${plural(plan.summary.deleteForEveryone, "message")} for everyone?`;

  return (
    <dialog
      ref={dialogRef}
      className={`confirm-dialog ${critical ? "is-critical" : ""}`}
      aria-labelledby={titleId}
      onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}
    >
      <div className="dialog-icon">{critical ? <AlertOctagon /> : <ShieldAlert />}</div>
      <button type="button" className="dialog-close" onClick={onClose} disabled={busy} aria-label="Close confirmation"><X size={18} /></button>
      <p className="eyebrow">{critical ? "CRITICAL ACTION" : "FINAL REVIEW"}</p>
      <h2 id={titleId}>{heading}</h2>
      <p className="dialog-lead">
        {operation === null ? "Only the ordered effects below are included in this frozen plan. Review each effect before continuing." : destroysChat
          ? "Telegram will remove every member and message, release public usernames, and dissolve this group. Retract cannot reverse it."
          : operation === "clear_history_and_leave"
            ? "Retract will ask Telegram to clear the complete history for everyone before leaving the group or channel and removing its remaining local entry. This is the broadest history cleanup Telegram currently reports as available; externally saved copies remain outside Telegram’s control."
          : operation === "delete_all_messages_and_leave"
            ? `Retract froze ${plural(plan.summary.selected, "message")} from every participant in this chat. It will recheck and delete every eligible message for everyone, including attached media and captions, then leave and remove the remaining local entry. Rejected or protected messages will be reported without a self-only fallback.`
          : operation === "leave_chat"
            ? plan.summary.deleteForEveryone > 0
              ? `Retract will first recheck and attempt to delete ${plural(plan.summary.deleteForEveryone, "message")} sent by your account for everyone, including attached media and captions. Messages Telegram no longer permits will be skipped or reported as failed. Retract will then leave the chat and remove its remaining local history; other participants’ messages and saved copies remain.`
              : "Retract found no messages sent by your account that Telegram currently permits it to revoke for everyone. It will leave the chat and remove the remaining history from your chat list; other participants’ copies remain."
          : operation === "clear_history"
            ? "Every message Telegram permits will disappear for all chat members, and the conversation will be removed from your chat list. A future DM can recreate the conversation; group membership remains unless the group itself is permanently deleted."
          : operation === "remove_chat_for_self"
            ? "Telegram will delete this chat history only for your account and remove the conversation from your chat list. The other participant or group members keep their copies. A future message can recreate the conversation."
            : operation === "delete_my_messages"
              ? "Only messages sent by your account in this group will be removed for everyone. Your membership and every other participant’s messages remain. Attached media and captions are removed with their message; externally saved copies are outside Telegram’s control."
            : operation === "delete_by_sender"
              ? `Telegram will remove every message sent by ${plan.targetSenderName} in “${plan.chatTitle}”. This can affect far more messages than the current search results.`
            : "Only the messages in this frozen plan that still pass a live capability check will be removed. Attached media and captions are removed with their message; externally saved copies are outside Telegram’s control."}
      </p>

      <ActionEffects descriptors={plan.steps.map(step => step.descriptor)} targetCounts={plan.steps.map(step => step.targets.length)} label="Ordered plan effects" />

      {(operation === "selected_messages" || operation === "delete_my_messages" || operation === "delete_all_messages_and_leave" || operation === "leave_chat") && (
        <div className="dialog-summary">
          <div><strong>{plan.summary.selected}</strong><span>reviewed messages</span></div>
          <div><strong>{plan.summary.deleteForEveryone}</strong><span>delete for everyone</span></div>
          <div><strong>{plan.summary.selfOnly}</strong><span>self-only skipped</span></div>
          <div><strong>{plan.summary.cannotDelete}</strong><span>protected skipped</span></div>
        </div>
      )}

      <div className="plan-binding">
        <LockKeyhole size={14} /> Plan <code>{plan.fingerprint.slice(0, 12)}</code> is frozen to {operation === "selected_messages" ? "the reviewed IDs" : operation === "delete_my_messages" ? "your message IDs in this chat" : operation === "delete_all_messages_and_leave" ? "every enumerated message ID and this chat" : operation === "leave_chat" ? "your outgoing message IDs and this chat" : operation === "clear_history_and_leave" ? "whole-history cleanup and this immutable chat" : operation === "delete_by_sender" ? "this chat and sender" : "this immutable chat"}.
      </div>

      {plan.confirmation.ownerAuthRequired && <p className="system-auth-note">macOS will show the exact frozen target and verify the device owner after you press the final button.</p>}

      {titleRequired && (
        <label className="typed-confirmation">
          Type <strong>{plan.confirmation.exactText}</strong> to continue
          <input
            value={typedTitle}
            onChange={(event) => setTypedTitle(event.target.value)}
            autoComplete="off"
            spellCheck={false}
            placeholder={plan.confirmation.exactText || "Required confirmation text"}
            autoFocus
          />
        </label>
      )}

      {plan.confirmation.acknowledgementRequired && <label className="irreversible-check">
        <input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
        <span className="custom-checkbox">{acknowledged && <Check size={12} />}</span>
        <span>{operation === "clear_history_and_leave" ? "I understand this attempts complete history revocation before removing my membership and local entry." : operation === "delete_all_messages_and_leave" ? "I understand Retract will attempt to revoke every frozen eligible message before leaving and removing my local entry." : operation === "leave_chat" ? "I understand Retract will try to revoke my frozen outgoing messages, then remove my membership and remaining copy." : operation === "remove_chat_for_self" ? "I understand this deletes only my history and chat-list entry, not anyone else’s copy." : "I understand that accepted Telegram deletions cannot be undone."}</span>
      </label>}

      <div className="dialog-actions">
        <button type="button" className="cancel-button" onClick={onClose} disabled={busy}>Cancel</button>
        <button type="button" className={`confirm-button ${critical ? "critical" : ""}`} disabled={!canConfirm} onClick={() => onConfirm(acknowledged, typedTitle || null)}>
          {leavesChat ? <LogOut size={16} /> : <Eraser size={16} />}
          {busy ? "Starting safely…" : operation === null ? "Confirm reviewed effects" : destroysChat ? "Delete group permanently" : operation === "clear_history_and_leave" ? "Clear all history & leave" : operation === "delete_all_messages_and_leave" ? "Delete all possible & leave" : operation === "leave_chat" ? "Revoke my messages & leave" : operation === "remove_chat_for_self" ? "Remove chat for me" : operation === "delete_my_messages" ? "Delete all my messages" : operation === "delete_by_sender" ? "Delete sender’s messages" : operation === "clear_history" ? "Revoke & remove chat" : "Delete for everyone"}
        </button>
      </div>
    </dialog>
  );
}
