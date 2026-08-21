import { CheckCircle2, KeyRound, LoaderCircle, LockKeyhole, QrCode, Settings2, Smartphone } from "lucide-react";
import QRCode from "qrcode";
import { useEffect, useMemo, useState } from "react";
import { api } from "@retract/api";
import type { AuthSnapshot } from "../types";
import { BrandLogo } from "./BrandLogo";

interface AuthGateProps {
  auth: AuthSnapshot;
  onRefresh: () => Promise<void>;
  onOpenSettings: () => void;
}

export function AuthGate({ auth, onRefresh, onOpenSettings }: AuthGateProps) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [qrImage, setQrImage] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    if (!auth.qrLink) {
      setQrImage(null);
      return;
    }
    void QRCode.toDataURL(auth.qrLink, {
      errorCorrectionLevel: "M",
      margin: 2,
      width: 232,
      color: { dark: "#16231f", light: "#ffffff" }
    }).then((data) => { if (!disposed) setQrImage(data); });
    return () => { disposed = true; };
  }, [auth.qrLink]);

  const field = useMemo(() => authField(auth), [auth]);

  const requestQr = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.requestQrAuth();
      await onRefresh();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!field || !value) return;
    setBusy(true);
    setError(null);
    try {
      await api.submitAuth(field.command, value);
      setValue("");
      await onRefresh();
    } catch (cause) {
      setError(errorMessage(cause));
      if (field.secret) setValue("");
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="auth-shell">
      <section className="auth-card" aria-live="polite">
        <div className="auth-brand">
          <BrandLogo /><strong>Retract</strong><span className="auth-local-pill">LOCAL</span>
          <button type="button" className="auth-settings-button" onClick={onOpenSettings} aria-label="Open connection settings"><Settings2 size={15} /></button>
        </div>
        <p className="eyebrow">PRIVATE TELEGRAM SESSION</p>
        <h1>{authHeading(auth)}</h1>
        <p className="auth-lead">{authLead(auth)}</p>

        {auth.stage === "waiting_for_password" && auth.hint && (
          <div className="auth-password-hint">
            <KeyRound size={15} />
            <span>Password hint: <strong>{auth.hint}</strong></span>
          </div>
        )}

        {auth.stage === "waiting_for_phone" && (
          <button type="button" className="qr-start-button" disabled={busy} onClick={requestQr}>
            <QrCode size={20} />
            <span><strong>Sign in with QR code</strong><small>Recommended · no phone number typed here</small></span>
          </button>
        )}

        {auth.stage === "waiting_for_other_device" && (
          <div className="qr-panel">
            {qrImage ? <img src={qrImage} alt="Telegram device-link QR code" /> : <LoaderCircle className="spin" size={28} />}
            <div>
              <strong>Only scan this inside Telegram</strong>
              <span>Settings → Devices → Link Desktop Device</span>
            </div>
          </div>
        )}

        {field && (
          <form className="auth-form" onSubmit={submit}>
            {auth.stage === "waiting_for_phone" && <div className="auth-divider"><span>or use your phone number</span></div>}
            <label>
              <span>{field.label}</span>
              <div className="auth-input">
                {field.secret ? <KeyRound size={17} /> : <Smartphone size={17} />}
                <input
                  autoFocus={auth.stage !== "waiting_for_phone"}
                  type={field.secret ? "password" : field.type}
                  inputMode={field.inputMode}
                  autoComplete={field.autoComplete}
                  value={value}
                  onChange={(event) => setValue(event.target.value)}
                  placeholder={field.placeholder}
                  maxLength={field.maxLength}
                />
              </div>
            </label>
            <button type="submit" className="auth-submit" disabled={busy || !value.trim()}>
              {busy ? <LoaderCircle className="spin" size={16} /> : <CheckCircle2 size={16} />}
              {busy && auth.stage === "waiting_for_password" ? "Checking password…" : "Continue"}
            </button>
          </form>
        )}

        {auth.stage === "initializing" || auth.stage === "logging_out" ? (
          <div className="auth-progress"><LoaderCircle className="spin" size={18} /> Waiting for the local engine…</div>
        ) : null}

        {auth.stage === "error" || auth.stage === "closed" ? (
          <div className="auth-error" role="alert">{auth.hint || "The Telegram session is unavailable."}</div>
        ) : null}
        {error && <div className="auth-error" role="alert">{error}</div>}

        <footer className="auth-privacy"><LockKeyhole size={15} /><span>Codes and passwords go directly from the Rust core to local TDLib and are never persisted by Retract.</span></footer>
      </section>
    </main>
  );
}

type AuthCommand = Parameters<typeof api.submitAuth>[0];

function authField(auth: AuthSnapshot): {
  command: AuthCommand;
  label: string;
  placeholder: string;
  type: "text" | "tel" | "email";
  inputMode: "text" | "tel" | "email" | "numeric";
  autoComplete: string;
  maxLength: number;
  secret: boolean;
} | null {
  switch (auth.stage) {
    case "waiting_for_phone": return { command: "submit_phone", label: "Phone number", placeholder: "+1 555 010 0200", type: "tel", inputMode: "tel", autoComplete: "tel", maxLength: 32, secret: false };
    case "waiting_for_email_address": return { command: "submit_email_address", label: "Email address", placeholder: "you@example.com", type: "email", inputMode: "email", autoComplete: "email", maxLength: 254, secret: false };
    case "waiting_for_email_code": return { command: "submit_email_code", label: "Email code", placeholder: "Code from Telegram", type: "text", inputMode: "numeric", autoComplete: "one-time-code", maxLength: 32, secret: true };
    case "waiting_for_code": return { command: "submit_code", label: "Telegram sign-in code", placeholder: "Your code", type: "text", inputMode: "numeric", autoComplete: "one-time-code", maxLength: 32, secret: true };
    case "waiting_for_password": return { command: "submit_password", label: "Two-step verification password", placeholder: "Enter your Telegram two-step password", type: "text", inputMode: "text", autoComplete: "current-password", maxLength: 256, secret: true };
    default: return null;
  }
}

function authLead(auth: AuthSnapshot): string {
  if (auth.stage === "waiting_for_password") {
    return "Telegram approved this device. Now enter the separate two-step verification password configured on your Telegram account.";
  }
  return auth.hint || "Your session and message database stay encrypted on this Mac.";
}

function authHeading(auth: AuthSnapshot): string {
  switch (auth.stage) {
    case "waiting_for_phone": return "Connect your account";
    case "waiting_for_other_device": return "Scan to authorize this Mac";
    case "waiting_for_code": return "Check your Telegram messages";
    case "waiting_for_password": return "Two-step verification";
    case "waiting_for_email_address": return "Email verification required";
    case "waiting_for_email_code": return "Enter the email code";
    case "error": return "Sign-in needs attention";
    case "closed": return "Session closed";
    default: return "Opening your private workspace";
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "Telegram rejected the sign-in step.";
}
