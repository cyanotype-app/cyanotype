import { useEffect, useId, useState } from "react";
import { api, type AdminAccount, type ResetLink, type SessionInfo } from "../api";

type Panel = null | "reset" | "sessions" | "delete";

const when = (iso: string) => new Date(iso).toLocaleString();

interface Props {
  account: AdminAccount;
  /** The signed-in admin's own id: they cannot disable or delete themselves (the server refuses it too). */
  meId?: string;
  onChanged: () => Promise<void>;
}

/** What an admin can do to an account: disable, reset the password, change the role, sign it out, delete it. */
export function AccountActions({ account: a, meId, onChanged }: Props) {
  const [panel, setPanel] = useState<Panel>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const mine = meId === a.id;
  const disabled = !!a.disabled_at;
  const deleting = a.deletion === "queued" || a.deletion === "running";

  async function run(what: () => Promise<unknown>, done: string | null) {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      await what();
      if (done) setNote(done);
      await onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "That did not work.");
    } finally {
      setBusy(false);
    }
  }
  const toggle = (p: Exclude<Panel, null>) => {
    setError(null);
    setNote(null);
    setPanel((cur) => (cur === p ? null : p));
  };

  if (deleting) {
    return (
      <p className="small muted" role="status">
        Being deleted: its photos and data are being removed in the background.
      </p>
    );
  }

  return (
    <details className="account-manage">
      <summary aria-label={`Manage ${a.email}`}>Manage</summary>
      {a.deletion === "failed" && (
        <p className="small health-bad" role="status">
          The last attempt to delete this account failed: {a.deletion_error ?? "unknown error"}. Delete it again to retry.
        </p>
      )}
      <div className="invite-actions">
        {!mine && (
          <button
            type="button"
            className="btn btn-quiet"
            disabled={busy}
            onClick={() =>
              void run(
                () => (disabled ? api.enableAccount(a.id) : api.disableAccount(a.id)),
                disabled ? "Enabled. It can sign in again and its API keys work." : "Disabled. It is signed out and cannot sign in until you enable it.",
              )
            }
          >
            {disabled ? "Enable account" : "Disable account"}
          </button>
        )}
        <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => toggle("reset")} aria-expanded={panel === "reset"}>
          Reset password
        </button>
        <button
          type="button"
          className="btn btn-quiet"
          disabled={busy}
          onClick={() =>
            void run(
              () => api.setAccountRole(a.id, a.role === "admin" ? "member" : "admin"),
              a.role === "admin" ? "No longer an administrator." : "Now an administrator.",
            )
          }
        >
          {a.role === "admin" ? "Remove admin" : "Make admin"}
        </button>
        <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => toggle("sessions")} aria-expanded={panel === "sessions"}>
          Sessions
        </button>
        {!mine && (
          <button type="button" className="btn btn-danger-quiet" disabled={busy} onClick={() => toggle("delete")} aria-expanded={panel === "delete"}>
            Delete account…
          </button>
        )}
      </div>
      <div role="alert" className="form-error">
        {error}
      </div>
      {note && (
        <p className="small" role="status">
          {note}
        </p>
      )}
      {panel === "reset" && <ResetPanel account={a} />}
      {panel === "sessions" && <SessionsPanel account={a} />}
      {panel === "delete" && (
        <DeletePanel
          account={a}
          onDone={async () => {
            setPanel(null);
            await onChanged();
          }}
        />
      )}
    </details>
  );
}

function ResetPanel({ account: a }: { account: AdminAccount }) {
  const [link, setLink] = useState<ResetLink | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const url = link ? `${window.location.origin}${link.path}` : "";

  async function make() {
    setBusy(true);
    setError(null);
    try {
      setLink(await api.accountResetLink(a.id));
      setCopied(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not make the link.");
    } finally {
      setBusy(false);
    }
  }
  async function copy() {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
    } catch {
      setError("Could not copy automatically. Select the link and copy it by hand.");
    }
  }

  return (
    <div className="account-panel">
      <p className="small muted">
        Makes a one-time link that lets {a.email} choose a new password, and signs them out everywhere when it is used. It works for
        24 hours. Send it to them yourself; the server sends no email. A new link replaces an older one.
      </p>
      {!link ? (
        <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void make()}>
          Make a reset link
        </button>
      ) : (
        <div className="new-key" aria-label="Reset link">
          <strong>Copy this link now. It is not shown again.</strong>
          <code>{url}</code>
          <button type="button" className="btn btn-primary" onClick={() => void copy()}>
            {copied ? "Copied" : "Copy link"}
          </button>
        </div>
      )}
      <div role="alert" className="form-error">
        {error}
      </div>
    </div>
  );
}

function SessionsPanel({ account: a }: { account: AdminAccount }) {
  const [items, setItems] = useState<SessionInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    api
      .accountSessions(a.id)
      .then((r) => live && setItems(r.items))
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : "Could not load the sessions."));
    return () => {
      live = false;
    };
  }, [a.id, tick]);

  async function signOut() {
    setBusy(true);
    setError(null);
    try {
      await api.revokeAccountSessions(a.id);
      setTick((t) => t + 1);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not sign the account out.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="account-panel">
      {items === null ? (
        <p className="muted small" role="status">
          Loading…
        </p>
      ) : items.length === 0 ? (
        <p className="small">No active sessions.</p>
      ) : (
        <ul className="session-list small">
          {items.map((s) => (
            <li key={s.id}>
              Signed in {when(s.created_at)}
              <span className="muted"> · expires {when(s.expires_at)}</span>
            </li>
          ))}
        </ul>
      )}
      {items !== null && items.length > 0 && (
        <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => void signOut()}>
          Sign out everywhere
        </button>
      )}
      <div role="alert" className="form-error">
        {error}
      </div>
    </div>
  );
}

function DeletePanel({ account: a, onDone }: { account: AdminAccount; onDone: () => Promise<void> }) {
  const id = useId();
  const [typed, setTyped] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const matches = typed.trim().toLowerCase() === a.email.toLowerCase();

  async function remove() {
    setBusy(true);
    setError(null);
    try {
      await api.deleteAccount(a.id, typed.trim());
      await onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not start the deletion.");
      setBusy(false);
    }
  }

  return (
    <div className="account-panel account-delete">
      <p>
        <strong>This permanently deletes {a.email}</strong> and everything they stored: every photo and its thumbnails (every version
        in the bucket), albums, share links, API keys and sessions. It cannot be undone. The account is disabled and signed out at
        once and the rest is removed in the background.
      </p>
      <label htmlFor={id}>Type the email address to confirm</label>
      <input
        id={id}
        value={typed}
        autoComplete="off"
        autoCapitalize="none"
        spellCheck={false}
        placeholder={a.email}
        onChange={(e) => setTyped(e.target.value)}
      />
      <div className="invite-actions">
        <button type="button" className="btn btn-danger" disabled={!matches || busy} onClick={() => void remove()}>
          Delete account permanently
        </button>
      </div>
      <div role="alert" className="form-error">
        {error}
      </div>
    </div>
  );
}
