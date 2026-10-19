import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import { api, type Invite, type IssuedInvite, type NewInvite } from "../../api";
import { gigabytesToBytes, inviteStatusLine, inviteUrl, limitsLine } from "../../adminText";

const when = (iso: string) => new Date(iso).toLocaleString();
const EXPIRY_CHOICES = [1, 3, 7, 14, 30, 90];

/** Create, list, re-issue (show the link again) and revoke registration invites. */
export function Invites() {
  const noteId = useId();
  const daysId = useId();
  const quotaId = useId();
  const dailyId = useId();
  const fileId = useId();
  const [items, setItems] = useState<Invite[] | null>(null);
  const [note, setNote] = useState("");
  const [days, setDays] = useState(7);
  const [quota, setQuota] = useState("");
  const [daily, setDaily] = useState("");
  const [maxFile, setMaxFile] = useState("");
  const [issued, setIssued] = useState<IssuedInvite | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setItems((await api.listInvites()).items);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load the invites.");
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  async function create(e: FormEvent) {
    e.preventDefault();
    const q = gigabytesToBytes(quota);
    const d = gigabytesToBytes(daily);
    const f = gigabytesToBytes(maxFile);
    if (q === undefined || d === undefined || f === undefined) {
      setError("Limits are amounts in GB greater than zero, e.g. 5 or 0.5. Leave a field empty for no limit.");
      return;
    }
    const body: NewInvite = { expires_in_days: days };
    if (note.trim()) body.note = note.trim();
    if (q !== null) body.quota_bytes = q;
    if (d !== null) body.daily_upload_bytes = d;
    if (f !== null) body.max_file_bytes = f;
    setBusy(true);
    setError(null);
    try {
      setIssued(await api.createInvite(body));
      setCopied(false);
      setNote("");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not create the invite.");
    } finally {
      setBusy(false);
    }
  }

  async function showLinkAgain(inv: Invite) {
    const warn = "A new link replaces the old one: the old link stops working. Continue?";
    if (!window.confirm(warn)) return;
    setError(null);
    try {
      setIssued(await api.reissueInvite(inv.id));
      setCopied(false);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not make a new link.");
    }
  }

  async function revoke(inv: Invite) {
    if (!window.confirm("Revoke this invite? Its link stops working.")) return;
    setError(null);
    try {
      await api.revokeInvite(inv.id);
      if (issued?.invite.id === inv.id) setIssued(null);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not revoke the invite.");
    }
  }

  const link = issued ? inviteUrl(window.location.origin, issued.path) : "";
  async function copy() {
    try {
      await navigator.clipboard.writeText(link);
      setCopied(true);
    } catch {
      setError("Could not copy automatically. Select the link and copy it by hand.");
    }
  }

  return (
    <section aria-labelledby="invites-title">
      <h2 id="invites-title">Invites</h2>
      <p className="muted">
        An invite is a single-use link that lets one person create an account. Send it to them yourself; the server sends no
        email.
      </p>

      <form className="invite-form" onSubmit={create}>
        <div className="field-wide">
          <label htmlFor={noteId}>Note (only you see it)</label>
          <input
            id={noteId}
            value={note}
            maxLength={200}
            placeholder="e.g. for Sam"
            onChange={(e) => setNote(e.target.value)}
          />
        </div>
        <div>
          <label htmlFor={daysId}>Expires after</label>
          <select id={daysId} value={days} onChange={(e) => setDays(Number(e.target.value))}>
            {EXPIRY_CHOICES.map((n) => (
              <option key={n} value={n}>
                {n} {n === 1 ? "day" : "days"}
              </option>
            ))}
          </select>
        </div>
        <details className="field-wide invite-limits">
          <summary>Starting limits (optional)</summary>
          <p className="muted small">Copied onto the new account when the invite is used. Empty means no limit.</p>
          <div className="invite-limit-fields">
            <div>
              <label htmlFor={quotaId}>Storage (GB)</label>
              <input id={quotaId} inputMode="decimal" value={quota} placeholder="No limit" onChange={(e) => setQuota(e.target.value)} />
            </div>
            <div>
              <label htmlFor={dailyId}>Uploads per day (GB)</label>
              <input id={dailyId} inputMode="decimal" value={daily} placeholder="No limit" onChange={(e) => setDaily(e.target.value)} />
            </div>
            <div>
              <label htmlFor={fileId}>Largest file (GB)</label>
              <input id={fileId} inputMode="decimal" value={maxFile} placeholder="No limit" onChange={(e) => setMaxFile(e.target.value)} />
            </div>
          </div>
        </details>
        <button type="submit" className="btn btn-primary" disabled={busy}>
          Create invite
        </button>
      </form>

      <div role="alert" className="form-error">
        {error}
      </div>

      {issued && (
        <section className="new-key" aria-label="Invite link">
          <strong>Copy this link now and send it. It is not shown again.</strong>
          <code>{link}</code>
          <p className="muted small">
            It works once and expires {new Date(issued.invite.expires_at).toLocaleDateString()}. If you lose it, make a new
            link from the list.
          </p>
          <button type="button" className="btn btn-primary" onClick={() => void copy()}>
            {copied ? "Copied" : "Copy link"}
          </button>{" "}
          <button type="button" className="btn btn-quiet" onClick={() => setIssued(null)}>
            Done
          </button>
        </section>
      )}

      {items === null ? (
        <p className="muted" role="status">
          Loading…
        </p>
      ) : items.length === 0 ? (
        <p className="muted">No invites yet.</p>
      ) : (
        <ul className="invite-list">
          {items.map((inv) => {
            const limits = limitsLine(inv.limits);
            return (
              <li key={inv.id}>
                <div className="invite-main">
                  <div className="invite-title">
                    <strong>{inv.note ?? "No note"}</strong>
                    <span className={`badge badge-${inv.status}`}>{inv.status}</span>
                  </div>
                  <div className="muted small">{inviteStatusLine(inv)}</div>
                  <div className="muted small">
                    Created {when(inv.created_at)}
                    {inv.created_by ? ` by ${inv.created_by}` : ""}
                    {inv.used_at ? ` · used ${when(inv.used_at)}` : ""}
                  </div>
                  {limits && <div className="muted small">Starts with {limits}</div>}
                </div>
                {(inv.status === "pending" || inv.status === "expired") && (
                  <div className="invite-actions">
                    <button type="button" className="btn btn-quiet" onClick={() => void showLinkAgain(inv)}>
                      {inv.status === "expired" ? "Renew link" : "Show link again"}
                    </button>
                    {inv.status === "pending" && (
                      <button type="button" className="btn btn-quiet" onClick={() => void revoke(inv)}>
                        Revoke
                      </button>
                    )}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
