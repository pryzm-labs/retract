import { CheckCircle2, Eye, EyeOff, Globe2, KeyRound, LoaderCircle, ShieldAlert, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "@retract/api";
import type { ActiveContext } from "../providers/identity";
import type { DiscordBrowser, DiscordSessionStatus } from "../types";

export function DiscordConnectionDialog({ context, onClose, onReady, onError }: { context: ActiveContext; onClose: () => void; onReady: (status: DiscordSessionStatus) => void; onError: (error: unknown) => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  const [session, setSession] = useState<DiscordSessionStatus | null>(null);
  const [browsers, setBrowsers] = useState<DiscordBrowser[]>([]);
  const [token, setToken] = useState("");
  const [reveal, setReveal] = useState(false);
  const [remember, setRemember] = useState(false);
  const [risk, setRisk] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => { ref.current?.showModal(); return () => setToken(""); }, []);
  useEffect(() => { void Promise.all([api.discordSession(context), api.discordBrowsers(context)]).then(([status, found]) => { setSession(status); setBrowsers(found); }).catch(onError); }, [context, onError]);
  async function automatic(browser: DiscordBrowser) {
    setBusy(true);
    try { const status = await api.connectDiscordBrowser(browser.id, remember, risk, context); setSession(status); onReady(status); }
    catch (error) { onError(error); }
    finally { setBusy(false); }
  }
  async function manual(event: React.FormEvent) {
    event.preventDefault();
    const submitted = token;
    setToken(""); setReveal(false); setBusy(true);
    try { const status = await api.submitDiscordToken(submitted, remember, risk, context); setSession(status); onReady(status); }
    catch (error) { onError(error); }
    finally { setBusy(false); }
  }
  async function forget() {
    setBusy(true);
    try { setSession(await api.forgetDiscordSession(context)); }
    catch (error) { onError(error); }
    finally { setBusy(false); }
  }
  return <dialog ref={ref} className="discord-connection-dialog" onCancel={event => { if (busy) event.preventDefault(); else onClose(); }}>
    <button className="dialog-close" type="button" aria-label="Close Discord connection" onClick={onClose} disabled={busy}><X size={18} /></button>
    <p className="eyebrow">DISCORD REMEDIATION</p><h2>Connect the matching Discord account</h2>
    {session?.state === "ready" ? <div className="discord-session-ready"><CheckCircle2 size={19} /><span><strong>{session.displayName || session.username}</strong><small>Verified for this archive{session.remembered ? " · stored in Keychain" : " · memory only"}</small></span><button type="button" onClick={() => void forget()} disabled={busy}><Trash2 size={14} />Forget</button></div> : <>
      <div className="risk-disclosure"><ShieldAlert size={19} /><p><strong>Discord does not officially support normal-user automation.</strong> Automated deletion may trigger rate limits or account enforcement. Retract only deletes exact messages attributed to you in the selected archive.</p></div>
      <label className="irreversible-check"><input type="checkbox" checked={risk} onChange={event => setRisk(event.target.checked)} /><span className="custom-checkbox" /><span>I understand the Discord account risk and want to continue.</span></label>
      {browsers.length > 0 && <section className="browser-options"><p className="eyebrow">ISOLATED BROWSER SIGN-IN</p><p>A fresh temporary browser profile opens. Retract does not inspect your normal browser profile.</p>{browsers.map(browser => <button type="button" key={browser.id} disabled={!risk || busy} onClick={() => void automatic(browser)}><Globe2 size={16} /><span>Continue with {browser.displayName}</span>{busy && <LoaderCircle className="spin" size={14} />}</button>)}{busy && <button type="button" className="cancel-browser-auth" onClick={() => void api.cancelDiscordBrowser(context)}>Cancel browser sign-in</button>}</section>}
      <form className="manual-token-form" onSubmit={manual}>
        <p className="eyebrow">MANUAL TOKEN — WORKS WITH ANY BROWSER</p>
        <p>Paste a current Discord user token. It is sent directly to the local Rust core and cleared from this field immediately.</p>
        <label><span>Discord token</span><span className="secret-input"><KeyRound size={15} /><input type={reveal ? "text" : "password"} value={token} onChange={event => setToken(event.target.value)} autoComplete="off" spellCheck={false} /><button type="button" onClick={() => setReveal(value => !value)} aria-label={reveal ? "Hide token" : "Reveal token"}>{reveal ? <EyeOff size={15} /> : <Eye size={15} />}</button></span></label>
        <label className="remember-secret"><input type="checkbox" checked={remember} onChange={event => setRemember(event.target.checked)} />Remember in macOS Keychain</label>
        <button className="confirm-button" type="submit" disabled={!risk || busy || token.length < 20}>{busy ? <LoaderCircle className="spin" size={15} /> : <KeyRound size={15} />}Verify token</button>
      </form>
      <details className="manual-token-help"><summary>How to copy a token manually</summary><ol><li>Sign in at discord.com/app in any browser.</li><li>Open that browser’s developer tools and select Network.</li><li>Reload Discord, open a request to <code>discord.com/api</code>, and copy the request’s <code>Authorization</code> header value.</li><li>Paste only that value above. Never paste cookies, passwords, or a bot token.</li></ol></details>
    </>}
  </dialog>;
}
