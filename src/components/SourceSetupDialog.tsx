import { Archive, LoaderCircle, MessageCircle, Upload, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "@retract/api";
import type { ActiveContext } from "../providers/identity";
import type { DiscordImportStatus, DiscordSource } from "../types";

interface Props {
  context: ActiveContext | null;
  telegramConfigured: boolean;
  required?: boolean;
  onClose: () => void;
  onTelegram: () => void;
  onSelect: (source: DiscordSource) => Promise<void>;
  onError: (error: unknown) => void;
}

export function SourceSetupDialog({ context, telegramConfigured, required = false, onClose, onTelegram, onSelect, onError }: Props) {
  const ref = useRef<HTMLDialogElement>(null);
  const [sources, setSources] = useState<DiscordSource[]>([]);
  const [status, setStatus] = useState<DiscordImportStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [sourceLookup, setSourceLookup] = useState<"loading" | "ready" | "failed">("loading");
  const [sourceLookupVersion, setSourceLookupVersion] = useState(0);
  const autoSelecting = useRef(false);
  const selecting = useRef(false);

  useEffect(() => { const dialog = ref.current; if (dialog && !dialog.open) dialog.showModal(); }, []);
  useEffect(() => {
    let disposed = false;
    setSourceLookup("loading");
    setSources([]);
    void api.discordImport(context).then(value => {
      if (disposed) return;
      setStatus(value);
      setSources(value.sources);
      setSourceLookup("ready");
    }).catch(error => {
      if (disposed || selecting.current) return;
      setSourceLookup("failed");
      onError(error);
    });
    return () => { disposed = true; };
  }, [context, onError, sourceLookupVersion]);
  useEffect(() => {
    if (!status?.active) return;
    let disposed = false;
    const timer = window.setInterval(() => {
      if (selecting.current) return;
      void api.discordImport(context).then(next => {
        if (!disposed && !selecting.current) void applyStatus(next);
      }).catch(error => { if (!disposed && !selecting.current) onError(error); });
    }, 400);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [status?.active, context, onError]);

  async function importArchive() {
    autoSelecting.current = false;
    setBusy(true);
    try { await applyStatus(await api.startDiscordImport(context)); }
    catch (error) { onError(error); }
    finally { setBusy(false); }
  }
  async function retryArchive() {
    autoSelecting.current = false;
    setBusy(true);
    try { await applyStatus(await api.retryDiscordImport(context)); }
    catch (error) { onError(error); }
    finally { setBusy(false); }
  }
  async function applyStatus(next: DiscordImportStatus) {
    setStatus(next);
    setSources(next.sources);
    setSourceLookup("ready");
    if (next.progress?.phase !== "ready" || !next.importScope || autoSelecting.current) return;
    const ready = next.sources.find(source => sameScope(source, next.importScope!));
    if (!ready) return;
    autoSelecting.current = true;
    await select(ready);
  }
  async function select(source: DiscordSource) {
    if (selecting.current) return;
    selecting.current = true;
    setBusy(true);
    try { await onSelect(source); }
    catch (error) {
      selecting.current = false;
      autoSelecting.current = false;
      onError(error);
      setBusy(false);
    }
  }
  const progress = status?.progress;
  const processed = !progress ? "" : progress.phase === "failed"
    ? `${progress.committedItems.toLocaleString()} staged records · not yet available`
    : progress.phase === "ready"
      ? `${progress.committedItems.toLocaleString()} messages ready`
      : progress.totalRecords
        ? `${progress.parsedRecords.toLocaleString()} of ${progress.totalRecords.toLocaleString()} messages processed`
        : `${progress.parsedRecords.toLocaleString()} messages processed`;

  return <dialog ref={ref} className="source-setup-dialog" onCancel={event => { if (required || busy) event.preventDefault(); else onClose(); }}>
    {!required && <button className="dialog-close" type="button" aria-label="Close source setup" onClick={onClose} disabled={busy}><X size={18} /></button>}
    <p className="eyebrow">DATA SOURCE</p>
    <h2>Choose what to clean up</h2>
    <p className="dialog-lead">Retract works locally with Telegram or an exported Discord data package. You can switch sources later.</p>
    <div className="source-choice-grid">
      <button type="button" className="source-choice" onClick={onTelegram} disabled={busy}>
        <MessageCircle size={22} /><span><strong>Telegram</strong><small>{telegramConfigured ? "Use the configured account" : "Configure a real account"}</small></span>
      </button>
      <button type="button" className="source-choice" onClick={() => void importArchive()} disabled={busy || status?.active}>
        {busy || status?.active ? <LoaderCircle className="spin" size={22} /> : <Upload size={22} />}
        <span><strong>Import Discord package</strong><small>Choose the ZIP downloaded from Discord</small></span>
      </button>
    </div>
    {progress && <div className={`discord-import-progress phase-${progress.phase}`} role="status" aria-live="polite">
      <Archive size={17} /><span><strong>{importPhase(progress.phase)}</strong><small>{processed}</small>{progress.warnings > 0 && <small>{progress.warnings.toLocaleString()} warnings</small>}</span>
      {status?.active && <button type="button" onClick={() => void api.cancelDiscordImport(context).then(setStatus).catch(onError)}>Cancel</button>}
      {status?.retryAvailable && <button type="button" onClick={() => void retryArchive()} disabled={busy}>Retry failed import</button>}
    </div>}
    {progress?.phase === "failed" && <div className="discord-import-details">
      <p>{failureMessage(status?.failureCode ?? null)}</p>
      {status?.warningDetails.map(warning => <p key={warning.code}>{warningMessage(warning.code, warning.count)}</p>)}
      {status?.retryAvailable && <small>Choose the same Discord ZIP to retry safely. Staged records will not be duplicated.</small>}
    </div>}
    <section className="available-sources">
      <p className="eyebrow">IMPORTED DISCORD ACCOUNTS</p>
      {sourceLookup === "loading" ? <div className="source-discovery-state" role="status" aria-label="Imported Discord accounts">
        <LoaderCircle className="spin" size={16} /><span>Loading imported accounts…</span>
      </div> : sourceLookup === "failed" ? <div className="source-discovery-state source-discovery-error" role="alert">
        <span>Couldn’t load imported accounts.</span>
        <button type="button" onClick={() => setSourceLookupVersion(version => version + 1)}>Retry loading imported accounts</button>
      </div> : sources.length === 0 ? <p className="source-discovery-empty">No imported Discord accounts yet.</p> : sources.map(source => <button type="button" key={source.scope.sourceId} onClick={() => void select(source)} disabled={busy}>
        <Archive size={17} /><span><strong>{source.accountLabel}</strong><small>{source.username ? `@${source.username} · ` : ""}{source.importedAt ? `Imported ${new Date(source.importedAt).toLocaleDateString()}` : "Ready"}</small></span>
      </button>)}
    </section>
    <p className="source-privacy-note">Archive contents stay on this device. Importing does not connect to Discord or delete anything.</p>
  </dialog>;
}

function importPhase(phase: NonNullable<DiscordImportStatus["progress"]>["phase"]): string {
  return ({ inspecting: "Inspecting archive", hashing: "Verifying package", registering: "Registering source", importing: "Importing messages", verifying: "Checking imported records", ready: "Archive ready", cancelled: "Import cancelled", failed: "Import failed" })[phase];
}

function sameScope(source: DiscordSource, scope: NonNullable<DiscordImportStatus["importScope"]>): boolean {
  return source.scope.provider === scope.provider && source.scope.accountId === scope.accountId && source.scope.sourceId === scope.sourceId;
}

function failureMessage(code: DiscordImportStatus["failureCode"]): string {
  return ({
    invalid_archive: "The package contains a record Retract could not safely import.",
    unsupported_profile: "This Discord export format is not supported yet.",
    limit_exceeded: "The package exceeds Retract's safe import limits.",
    input_changed: "The selected ZIP changed while Retract was reading it.",
    incomplete_source: "The package ended before all required records were available.",
    storage_failure: "Retract could not finish writing the local archive."
  } as const)[code ?? "invalid_archive"];
}

function warningMessage(code: string, count: number): string {
  if (code === "unknown_conversation_kind") return `${count.toLocaleString()} ${count === 1 ? "conversation has" : "conversations have"} an unverified type and will remain searchable after a successful import.`;
  return `${count.toLocaleString()} non-blocking import ${count === 1 ? "warning was" : "warnings were"} recorded.`;
}
