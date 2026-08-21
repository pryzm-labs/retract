import {
  AlertTriangle,
  CheckCircle2,
  ChevronRight,
  CircleSlash2,
  Eraser,
  History,
  Info,
  LogOut,
  LoaderCircle,
  RotateCcw,
  ShieldCheck,
  Trash2,
  Users
} from "lucide-react";
import type { ChatSummary, JobRecord, MessageSnapshot, PlanOperation } from "../types";
import { plural, roleLabel } from "./common";

interface ImpactPanelProps {
  selected: MessageSnapshot[];
  activeChat?: ChatSummary;
  jobs: JobRecord[];
  runtimeMode: string;
  busy: boolean;
  busyLabel: string | null;
  chatRemovalPending: boolean;
  hiddenSelectionCount: number;
  onReview: () => void;
  onChatAction: (operation: PlanOperation) => void;
  onOwnMessagesAction: () => void;
  onSenderAction: (sender: MessageSnapshot) => void;
  onClearSelection: () => void;
  onCancelJob: (jobId: string) => void;
  onResetDemo: () => void;
}

export function ImpactPanel({ selected, activeChat, jobs, runtimeMode, busy, busyLabel, chatRemovalPending, hiddenSelectionCount, onReview, onChatAction, onOwnMessagesAction, onSenderAction, onClearSelection, onCancelJob, onResetDemo }: ImpactPanelProps) {
  const everyone = selected.filter((message) => message.deletionReach === "everyone").length;
  const selfOnly = selected.filter((message) => message.deletionReach === "self_only").length;
  const blocked = selected.filter((message) => message.deletionReach === "none").length;
  const recentJobs = jobs.slice(0, 3);
  const senderCandidate = activeChat
    && selected.length > 0
    && selected.every((message) => message.chatId === activeChat.id && message.senderId === selected[0].senderId)
      ? selected[0]
      : undefined;
  const canDeleteOwnHistory = activeChat
    && (activeChat.kind === "basic_group" || activeChat.kind === "supergroup")
    && (activeChat.capabilities.role !== "member" || activeChat.capabilities.canLeaveChat)
    && activeChat.conversationState !== "empty"
    && activeChat.conversationState !== "never_replied";

  return (
    <aside className="impact-panel" aria-label="Selection impact">
      <div className="impact-scroll">
        <section className="impact-card primary-impact">
          <p className="eyebrow">SELECTION IMPACT</p>
          <div className="selection-total">
            <strong>{selected.length.toLocaleString()}</strong>
            <span>{selected.length === 1 ? "message selected" : "messages selected"}</span>
          </div>
          <div className="impact-breakdown">
            <ImpactRow icon={<ShieldCheck />} tone="green" count={everyone} label="Delete for everyone" />
            <ImpactRow icon={<Info />} tone="amber" count={selfOnly} label="Only removable for you" />
            <ImpactRow icon={<CircleSlash2 />} tone="gray" count={blocked} label="Cannot delete" />
          </div>
          {selfOnly > 0 && <p className="impact-explanation">Self-only items will be skipped. They are never used as a fallback.</p>}
          {hiddenSelectionCount > 0 && (
            <div className="hidden-selection" role="status">
              <span>{plural(hiddenSelectionCount, "selection")} outside this result view</span>
              <button type="button" onClick={onClearSelection}>Clear all</button>
            </div>
          )}
          <button className="review-button" type="button" disabled={everyone === 0 || busy} onClick={onReview}>
            <Eraser size={17} />
            Review deletion
            <ChevronRight size={16} />
          </button>
          {busyLabel && <p className="impact-busy" role="status"><LoaderCircle className="spin" size={12} />{busyLabel}</p>}
        </section>

        {activeChat && (
          <section className="impact-card chat-authority-card">
            <div className="card-title-row">
              <div>
                <p className="eyebrow">CHAT AUTHORITY</p>
                <h2>{activeChat.title}</h2>
              </div>
              {activeChat.memberCount && <span className="member-count"><Users size={13} />{activeChat.memberCount.toLocaleString()}</span>}
            </div>
            <p className={`authority-role role-${activeChat.capabilities.role}`}>{roleLabel(activeChat.capabilities.role)}</p>
            {activeChat.conversationState === "empty" && (
              <p className="candidate-note"><Info size={14} />Telegram returned no message history for this account. There is nothing to revoke, but Retract can remove the conversation from your chat list when the capability is available.</p>
            )}
            {activeChat.conversationState === "never_replied" && (
              <p className="candidate-note"><AlertTriangle size={14} />No messages sent by your account were found. Review the sender and title before treating this chat as spam.</p>
            )}
            {activeChat.conversationState === "awaiting_reply" && (
              <p className="candidate-note"><Info size={14} />The latest message is incoming, but you have sent messages here before.</p>
            )}
            {chatRemovalPending && (
              <p className="chat-removal-status" role="status"><LoaderCircle className="spin" size={14} />Waiting for Telegram to finish removing this chat…</p>
            )}
            <ul className="capability-list">
              <li className={activeChat.capabilities.canDeleteOthers ? "yes" : "no"}>
                {activeChat.capabilities.canDeleteOthers ? <CheckCircle2 /> : <CircleSlash2 />}
                Delete other people’s messages
              </li>
              <li className={activeChat.capabilities.canClearForEveryone ? "yes" : "no"}>
                {activeChat.capabilities.canClearForEveryone ? <CheckCircle2 /> : <CircleSlash2 />}
                Clear history for everyone
              </li>
              <li className={activeChat.capabilities.canRemoveForSelf ? "yes" : "no"}>
                {activeChat.capabilities.canRemoveForSelf ? <CheckCircle2 /> : <CircleSlash2 />}
                Delete history and remove for me
              </li>
              <li className={activeChat.capabilities.canDeleteGroup ? "yes" : "no"}>
                {activeChat.capabilities.canDeleteGroup ? <CheckCircle2 /> : <CircleSlash2 />}
                Permanently delete group
              </li>
              <li className={activeChat.capabilities.canLeaveChat ? "yes" : "no"}>
                {activeChat.capabilities.canLeaveChat ? <CheckCircle2 /> : <CircleSlash2 />}
                Leave and remove for me
              </li>
            </ul>
            {activeChat.capabilities.canLeaveChat && (
              <button type="button" className="chat-action cleanup-primary" onClick={() => onChatAction("leave_chat")} disabled={busy || chatRemovalPending}>
                {activeChat.capabilities.canClearForEveryone
                  ? <><History size={15} /> Clear all history &amp; leave {activeChat.kind === "channel" ? "channel" : "group"}</>
                  : activeChat.capabilities.canDeleteOthers
                    ? <><Eraser size={15} /> Delete all possible history &amp; leave {activeChat.kind === "channel" ? "channel" : "group"}</>
                    : <><LogOut size={15} /> Revoke my messages &amp; leave {activeChat.kind === "channel" ? "channel" : "group"}</>}
                <ChevronRight size={15} />
              </button>
            )}
            {activeChat.capabilities.canClearForEveryone && (
              <button type="button" className="chat-action" onClick={() => onChatAction("clear_history")} disabled={busy || chatRemovalPending}>
                <History size={15} /> Revoke history &amp; remove chat <ChevronRight size={15} />
              </button>
            )}
            {activeChat.capabilities.canRemoveForSelf && (
              <button type="button" className="chat-action" onClick={() => onChatAction("remove_chat_for_self")} disabled={busy || chatRemovalPending}>
                {chatRemovalPending ? <LoaderCircle className="spin" size={15} /> : <Trash2 size={15} />} {chatRemovalPending ? "Removing chat…" : activeChat.conversationState === "empty" ? "Remove chat from my list" : "Delete history & remove for me"} <ChevronRight size={15} />
              </button>
            )}
            {canDeleteOwnHistory && (
              <button type="button" className="chat-action" onClick={onOwnMessagesAction} disabled={busy || chatRemovalPending}>
                <Eraser size={15} /> Delete all my messages <ChevronRight size={15} />
              </button>
            )}
            {activeChat.capabilities.canDeleteBySender && senderCandidate && (
              <button type="button" className="chat-action" onClick={() => onSenderAction(senderCandidate)} disabled={busy || chatRemovalPending}>
                <Users size={15} /> Delete every message by {senderCandidate.senderName} <ChevronRight size={15} />
              </button>
            )}
            {activeChat.capabilities.canDeleteGroup && (
              <button type="button" className="chat-action danger" onClick={() => onChatAction("delete_group")} disabled={busy || chatRemovalPending}>
                <Trash2 size={15} /> Delete group permanently <ChevronRight size={15} />
              </button>
            )}
          </section>
        )}

        <section className="impact-card jobs-card">
          <div className="card-title-row">
            <div>
              <p className="eyebrow">RECENT CLEANUPS</p>
              <h2>Job activity</h2>
            </div>
          </div>
          {recentJobs.length === 0 ? (
            <p className="empty-jobs">No deletions have run in this local profile.</p>
          ) : recentJobs.map((job) => <JobRow key={job.id} job={job} busy={busy} onCancel={onCancelJob} />)}
          <p className="job-log-note"><ShieldCheck size={13} />Job logs contain IDs and counts, not message content.</p>
        </section>
      </div>

      {runtimeMode === "demo" && (
        <button type="button" className="reset-demo" onClick={onResetDemo} disabled={busy}>
          <RotateCcw size={13} /> Reset demo fixtures
        </button>
      )}
    </aside>
  );
}

function ImpactRow({ icon, tone, count, label }: { icon: React.ReactNode; tone: string; count: number; label: string }) {
  return <div className={`impact-row tone-${tone}`}><span className="impact-icon">{icon}</span><span>{label}</span><strong>{count.toLocaleString()}</strong></div>;
}

function JobRow({ job, busy, onCancel }: { job: JobRecord; busy: boolean; onCancel: (jobId: string) => void }) {
  const active = job.status === "queued" || job.status === "running";
  return (
    <div className="job-row">
      <span className={`job-state job-${job.status}`}>{active ? <History size={14} /> : job.status === "completed" ? <CheckCircle2 size={14} /> : <AlertTriangle size={14} />}</span>
      <span className="job-copy">
        <strong>{job.operation === "selected_messages" ? plural(job.total, "message") : job.operation === "delete_my_messages" ? `${job.total.toLocaleString()} of my messages` : job.operation === "clear_history_and_leave" ? "Clear all history & leave" : job.operation === "delete_all_messages_and_leave" ? "Delete all possible history & leave" : job.operation === "leave_chat" ? "Revoke my messages & leave" : job.operation === "remove_chat_for_self" ? "Remove chat for me" : job.operation.replaceAll("_", " ")}</strong>
        <small>{job.retryAfterSeconds ? `rate limited · retry in ${job.retryAfterSeconds}s` : job.status}{job.deleted > 0 ? ` · ${job.deleted} deleted` : ""}</small>
      </span>
      {active && <button type="button" className="job-cancel" disabled={busy} onClick={() => onCancel(job.id)}>Cancel</button>}
    </div>
  );
}
