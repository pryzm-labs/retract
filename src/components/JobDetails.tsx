import { useEffect, useId, useRef, useState } from "react";
import { X } from "lucide-react";
import type { JobRecord } from "../types";
import type { LegacyHistoryRecord } from "../providers/contract";
import { diagnosticMessage } from "./ActionEffects";

export function JobDetails({ job, onClose }: { job: JobRecord | LegacyHistoryRecord; onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const [now, setNow] = useState(Date.now);
  const scoped = "counters" in job;
  const retryAt = scoped ? job.retryAt : null;
  useEffect(() => {
    const previous = document.activeElement;
    dialogRef.current?.showModal();
    closeRef.current?.focus();
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, []);
  useEffect(() => {
    if (!retryAt) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [retryAt]);
  const seconds = retryAt ? Math.max(0, Math.ceil((Date.parse(retryAt) - now) / 1000)) : 0;
  const counts = scoped ? job.counters : { selected: null, eligible: job.total, deleted: job.deleted, skipped: job.skipped, failed: job.failed, uncertain: null };
  return <dialog ref={dialogRef} className="confirm-dialog job-details" aria-labelledby={titleId}
    onCancel={event => { event.preventDefault(); onClose(); }}
    onKeyDown={event => { if (event.key === "Escape") { event.preventDefault(); onClose(); } }}>
    <button ref={closeRef} type="button" className="dialog-close" aria-label="Close job details" onClick={onClose}><X size={18} /></button>
    <p className="eyebrow">RECENT CLEANUPS</p>
    <h2 id={titleId}>Cleanup job details</h2>
    <p role="status" aria-label="Job status">{job.status}</p>
    <div className="dialog-summary job-counts">
      {Object.entries(counts).map(([key, value]) => {
        const label = key[0].toUpperCase() + key.slice(1);
        return <div key={key} role="group" aria-label={label}><strong>{value === null ? "Not recorded" : value.toLocaleString()}</strong><span>{label}</span></div>;
      })}
    </div>
    {retryAt && <p role="status" aria-label="Retry state">{seconds > 0 ? `Retry in ${seconds} ${seconds === 1 ? "second" : "seconds"}` : "Waiting for provider update"}</p>}
    {job.status === "blocked" && <p className="dialog-lead">Reconnect the original account and source in Connection settings. Review this cleanup again before starting a new action; switching accounts does not move this job.</p>}
    {!scoped && <p className="dialog-lead">Legacy history is preserved. A new review is required. Original account binding and unrecorded counters cannot be recovered from this history.</p>}
    {job.diagnostics.length > 0 && <ul className="job-diagnostics" aria-label="Job diagnostics">{job.diagnostics.map((diagnostic, index) => <li key={index}>{diagnosticMessage(diagnostic.code)}</li>)}</ul>}
    <p className="dialog-lead">Deleted counts confirmed removals. Skipped items were not executed; failed and uncertain outcomes are separate. In-flight work may still have taken effect after cancellation or a connection change. Completed effects cannot be undone.</p>
  </dialog>;
}
