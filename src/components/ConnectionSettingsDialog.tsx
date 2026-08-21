import { Check, Database, Hash, KeyRound, LoaderCircle, LockKeyhole, MonitorCog, PackageCheck, RotateCw, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { ConnectionSettings, SaveConnectionSettingsResult } from "../types";

interface ConnectionSettingsDialogProps {
  settings: ConnectionSettings;
  required: boolean;
  onClose: () => void;
  onSaved: (result: SaveConnectionSettingsResult) => void;
}

export function ConnectionSettingsDialog({ settings, required, onClose, onSaved }: ConnectionSettingsDialogProps) {
  const [tdlibPath, setTdlibPath] = useState(settings.tdlibPath || settings.detectedTdlibPath || "");
  const [apiId, setApiId] = useState(settings.apiId?.toString() || "");
  const [apiHash, setApiHash] = useState("");
  const [useTestDc, setUseTestDc] = useState(settings.useTestDc);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const result = await api.saveConnectionSettings({
        tdlibPath,
        apiId: apiId.trim() ? Number(apiId) : null,
        apiHash: apiHash.trim() || null,
        useTestDc
      });
      setSaved(true);
      onSaved(result);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Retract could not save these settings.");
      setBusy(false);
    }
  };

  const hashReady = settings.apiHashConfigured || /^[a-fA-F0-9]{32}$/.test(apiHash.trim());
  const saveReady = Boolean(tdlibPath.trim() && Number(apiId) > 0 && hashReady);
  const environmentControlled = settings.environmentOverrides.length > 0;
  const tdlibEnvironmentOverride = settings.environmentOverrides.includes("RETRACT_TDLIB_PATH");

  return (
    <dialog
      ref={dialogRef}
      className="connection-dialog"
      onCancel={(event) => {
        event.preventDefault();
        if (!required && !busy) onClose();
      }}
    >
      {!required && <button type="button" className="dialog-close" onClick={onClose} disabled={busy} aria-label="Close settings"><X size={18} /></button>}
      <div className="connection-heading">
        <span className="connection-icon"><MonitorCog size={21} /></span>
        <div>
          <p className="eyebrow">{required ? "FIRST-RUN SETUP" : "CONNECTION SETTINGS"}</p>
          <h2>{required ? "Connect Telegram" : "Telegram connection"}</h2>
        </div>
      </div>
      <p className="connection-lead">
        Configure this once in Retract. The API hash is stored in macOS Keychain; sign-in codes, passwords, and message contents are never saved in these settings.
      </p>

      {environmentControlled && (
        <div className="environment-notice" role="status">
          <MonitorCog size={16} />
          <span><strong>Developer overrides are active.</strong> {settings.environmentOverrides.join(", ")} will take precedence when settings are applied.</span>
        </div>
      )}

      {settings.configurationError && (
        <div className="connection-error" role="alert">
          <span><strong>The previous settings could not be read.</strong> Review the fields and save to replace them. {settings.configurationError}</span>
        </div>
      )}

      <form onSubmit={save}>
        <div className="connection-fields">
            {settings.bundledTdlibAvailable && !tdlibEnvironmentOverride ? (
              <div className="bundled-tdlib">
                <PackageCheck size={19} />
                <span><strong>TDLib {settings.supportedTdlibVersion} included</strong><small>The pinned, self-contained library is selected automatically.</small></span>
                <span className="ready-pill">READY</span>
              </div>
            ) : (
              <label>
                <span className="field-label"><Database size={14} /> TDLib library <small>version {settings.supportedTdlibVersion}</small></span>
                <input
                  value={tdlibPath}
                  onChange={(event) => setTdlibPath(event.target.value)}
                  placeholder="/absolute/path/to/libtdjson.dylib"
                  autoComplete="off"
                  spellCheck={false}
                />
                {settings.detectedTdlibPath && tdlibPath === settings.detectedTdlibPath && <small className="field-success">Auto-detected on this Mac</small>}
              </label>
            )}

            <div className="credential-grid">
              <label>
                <span className="field-label"><Hash size={13} /> Telegram API ID</span>
                <input type="text" value={apiId} onChange={(event) => setApiId(event.target.value.replace(/\D/g, ""))} inputMode="numeric" placeholder="12345678" autoComplete="off" />
              </label>
              <label>
                <span className="field-label"><KeyRound size={13} /> Telegram API hash</span>
                <input
                  type="password"
                  value={apiHash}
                  onChange={(event) => setApiHash(event.target.value)}
                  placeholder={settings.apiHashConfigured ? "Stored securely — leave blank to keep" : "32-character API hash"}
                  autoComplete="new-password"
                  spellCheck={false}
                />
              </label>
            </div>
            <p className="credential-help">Obtain both values from <strong>my.telegram.org → API development tools</strong>. The hash never enters browser storage or the settings JSON file.</p>

            <label className="test-dc-toggle">
              <input type="checkbox" checked={useTestDc} onChange={(event) => setUseTestDc(event.target.checked)} />
              <span className="custom-checkbox">{useTestDc && <Check size={12} />}</span>
              <span><strong>Use Telegram’s test server</strong><small>Separate disposable accounts and database. Recommended for the destructive integration gate, but not for testing with a normal Telegram contact.</small></span>
            </label>
        </div>

        {error && <div className="connection-error" role="alert">{error}</div>}
        {saved && <div className="connection-restarting" role="status"><Check size={16} /> Settings applied.</div>}

        <footer className="connection-footer">
          <span className="keychain-note"><LockKeyhole size={14} /> Secrets stay in Keychain</span>
          {!required && <button type="button" className="cancel-button" disabled={busy} onClick={onClose}>Cancel</button>}
          <button type="submit" className="confirm-button" disabled={!saveReady || busy}>
            {busy ? <LoaderCircle className="spin" size={15} /> : <RotateCw size={15} />}
            {saved ? "Applied" : "Save settings"}
          </button>
        </footer>
      </form>
    </dialog>
  );
}
