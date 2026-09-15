import { Archive, LoaderCircle, MessageCircle, Upload, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "@retract/api";
import type { ActiveContext } from "../providers/identity";
import type { AppSnapshot, DiscordImportStatus, DiscordSource } from "../types";

interface Props {
  context: ActiveContext | null;
  telegramConfigured: boolean;
  required?: boolean;
  onClose: () => void;
  onTelegram: () => void;
  onSelected: (snapshot: AppSnapshot) => void;
  onError: (error: unknown) => void;
}

export function SourceSetupDialog({ context, telegramConfigured, required = false, onClose, onTelegram, onSelected, onError }: Props) {
  const ref = useRef<HTMLDialogElement>(null);
  const [sources, setSources] = useState<DiscordSource[]>([]);
  const [status, setStatus] = useState<DiscordImportStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => { const dialog = ref.current; if (dialog && !dialog.open) dialog.showModal(); }, []);
  useEffect(() => {
    let disposed = false;
    void api.discordSources(context).then(value => { if (!disposed) setSources(value); }).catch(onError);
    return () => { disposed = true; };
  }, [context, onError]);
  useEffect(() => {
    if (!status?.active) return;
    const timer = window.setInterval(() => void api.discordImport(context).then(next => {
      setStatus(next); setSources(next.sources);
    }).catch(onError), 400);
    return () => window.clearInterval(timer);
  }, [status?.active, context, onError]);

  async function importArchive() {
    setBusy(true);
    try { const next = await api.startDiscordImport(context); setStatus(next); setSources(next.sources); }
    catch (error) { onError(error); }
    finally { setBusy(false); }
  }
  async function select(source: DiscordSource) {
    setBusy(true);
    try { onSelected(await api.selectDiscordSource(source.scope, context)); }
    catch (error) { onError(error); setBusy(false); }
  }
  const progress = status?.progress;
  const processed = progress?.totalRecords ? `${progress.parsedRecords.toLocaleString()} of ${progress.totalRecords.toLocaleString()} messages` : progress ? `${progress.parsedRecords.toLocaleString()} messages` : "";

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
      <Archive size={17} /><span><strong>{importPhase(progress.phase)}</strong><small>{processed}{progress.warnings ? ` · ${progress.warnings} warnings` : ""}</small></span>
      {status?.active && <button type="button" onClick={() => void api.cancelDiscordImport(context).then(setStatus).catch(onError)}>Cancel</button>}
    </div>}
    {sources.length > 0 && <section className="available-sources">
      <p className="eyebrow">IMPORTED DISCORD ACCOUNTS</p>
      {sources.map(source => <button type="button" key={source.scope.sourceId} onClick={() => void select(source)} disabled={busy}>
        <Archive size={17} /><span><strong>{source.accountLabel}</strong><small>{source.username ? `@${source.username} · ` : ""}{source.importedAt ? `Imported ${new Date(source.importedAt).toLocaleDateString()}` : "Ready"}</small></span>
      </button>)}
    </section>}
    <p className="source-privacy-note">Archive contents stay on this device. Importing does not connect to Discord or delete anything.</p>
  </dialog>;
}

function importPhase(phase: NonNullable<DiscordImportStatus["progress"]>["phase"]): string {
  return ({ inspecting: "Inspecting archive", hashing: "Verifying package", registering: "Registering source", importing: "Importing messages", verifying: "Checking imported records", ready: "Archive ready", cancelled: "Import cancelled", failed: "Import failed" })[phase];
}
