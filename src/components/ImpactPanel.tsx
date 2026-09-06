import { sameScope, type Uuid } from "../providers/identity";
import { useState } from "react";
import type { IntentDescriptor, LegacyHistoryRecord } from "../providers/contract";
import { ActionEffects } from "./ActionEffects";
import { JobDetails } from "./JobDetails";
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
  legacyHistory?: LegacyHistoryRecord[];
  busy: boolean;
  busyLabel: string | null;
  chatRemovalPending: boolean;
  hiddenSelectionCount: number;
  onReview: () => void;
  onChatAction: (operation: PlanOperation) => void;
  onOwnMessagesAction: () => void;
  onSenderAction: (sender: MessageSnapshot) => void;
  onClearSelection: () => void;
  onCancelJob: (jobId: Uuid) => void;
}

export function ImpactPanel({ selected, activeChat, jobs, legacyHistory = [], busy, busyLabel, chatRemovalPending, hiddenSelectionCount, onReview, onChatAction, onOwnMessagesAction, onSenderAction, onClearSelection, onCancelJob }: ImpactPanelProps) {
  const [detailId, setDetailId] = useState<Uuid | null>(null);
  const detailJob = [...jobs, ...legacyHistory].find(job => job.id === detailId);
  const everyone = selected.filter((message) => message.deletionReach === "everyone").length;
  const selfOnly = selected.filter((message) => message.deletionReach === "self_only").length;
  const blocked = selected.filter((message) => message.deletionReach === "none").length;
  const recentJobs = jobs.slice(0, 3);
  const senderCandidate = activeChat
    && selected.length > 0
    && selected.every((message) => sameScope(message.scope, activeChat.scope) && message.chatId === activeChat.id && message.senderId === selected[0].senderId)
      ? selected[0]
      : undefined;
  function handler(intent: IntentDescriptor): (() => void) | undefined {
    if (intent.requiresActor) return intent.actionId === "delete_by_sender" && senderCandidate?.actorRef ? () => onSenderAction(senderCandidate) : undefined;
    switch (intent.actionId) {
      case "delete_my_messages": return onOwnMessagesAction;
      case "clear_history": case "clear_history_and_leave": case "delete_all_messages_and_leave": case "remove_chat_for_self": case "delete_group": case "leave_chat": {
        const operation = intent.actionId;
        return () => onChatAction(operation);
      }
      default: return undefined;
    }
  }

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
            {activeChat.intents.map(intent => {
              const onAction = handler(intent);
              const available = intent.descriptors.length > 0 && intent.descriptors.every(d => d.availability === "executable" || d.availability === "live_preflight_required");
              const critical = intent.descriptors.some(d => d.confirmationTier === "critical");
              return <div className="intent-action" key={intent.actionId}>
                <button type="button" className={`chat-action ${critical ? "danger" : intent.descriptors.length > 1 ? "cleanup-primary" : ""}`}
                  onClick={onAction} disabled={busy || chatRemovalPending || !available || !onAction}>
                  {critical ? <Trash2 size={15} /> : <Eraser size={15} />}<span>{intent.label}</span><ChevronRight size={15} />
                </button>
                {critical && <p className="critical-action-label">Critical action</p>}
                <ActionEffects descriptors={intent.descriptors} label={`${intent.label} effects`} />
                {intent.requiresActor && <p className="candidate-note">{senderCandidate?.actorRef ? `Selected sender: ${senderCandidate.senderName}` : "Select messages from one sender to review this action."}</p>}
              </div>;
            })}
          </section>
        )}

        <section className="impact-card jobs-card">
          <div className="card-title-row">
            <div>
              <p className="eyebrow">RECENT CLEANUPS</p>
              <h2>Job activity</h2>
            </div>
          </div>
          {recentJobs.length === 0 && legacyHistory.length === 0 ? (
            <p className="empty-jobs">No deletions have run in this local profile.</p>
          ) : recentJobs.map((job) => <JobRow key={job.id} job={job} busy={busy} onCancel={onCancelJob} onDetails={() => setDetailId(job.id)} />)}
          {legacyHistory.slice(0, 3).map(job => <div className="job-row" key={job.id}>
            <span className="job-copy"><strong>Legacy cleanup</strong><small>New review required</small></span>
            <button type="button" className="job-cancel" aria-label={`View details for legacy cleanup ${job.id}`} onClick={() => setDetailId(job.id)}>Details</button>
          </div>)}
          <p className="job-log-note"><ShieldCheck size={13} />Job logs contain IDs and counts, not message content.</p>
        </section>
      </div>

      {detailJob && <JobDetails key={detailJob.id} job={detailJob} onClose={() => setDetailId(null)} />}
    </aside>
  );
}

function ImpactRow({ icon, tone, count, label }: { icon: React.ReactNode; tone: string; count: number; label: string }) {
  return <div className={`impact-row tone-${tone}`}><span className="impact-icon">{icon}</span><span>{label}</span><strong>{count.toLocaleString()}</strong></div>;
}

function JobRow({ job, busy, onCancel, onDetails }: { job: JobRecord; busy: boolean; onCancel: (jobId: Uuid) => void; onDetails: () => void }) {
  const active = job.status === "queued" || job.status === "running";
  return (
    <div className="job-row" role="group" aria-label={`Cleanup job ${job.id}`}>
      <span className={`job-state job-${job.status}`}>{active ? <History size={14} /> : job.status === "completed" ? <CheckCircle2 size={14} /> : <AlertTriangle size={14} />}</span>
      <span className="job-copy">
        <strong>{plural(job.total, "message")}</strong>
        <small>{job.retryAfterSeconds ? `rate limited · retry in ${job.retryAfterSeconds}s` : job.status}{job.deleted > 0 ? ` · ${job.deleted} deleted` : ""}</small>
      </span>
      {active && <button type="button" className="job-cancel" disabled={busy} onClick={() => onCancel(job.id)}>Cancel</button>}
      <button type="button" className="job-cancel" aria-label={`View details for cleanup job ${job.id}`} aria-haspopup="dialog" onClick={onDetails}>Details</button>
    </div>
  );
}
