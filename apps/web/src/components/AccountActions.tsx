import { ChevronDown } from "lucide-react";
import { useEffect, useId, useState, type ReactNode } from "react";
import { api, type AccountApiKey, type AdminAccount, type ResetLink, type SessionInfo } from "../api";
import { SCOPE_LABEL } from "../twoFactorText";
import { FormError, inputClass, Loading, Mono, SecretBox, StatusNote } from "../settings/kit";
import { Button } from "../ui/Button";
import { ConfirmDialog } from "./ConfirmDialog";

type Panel = null | "reset" | "sessions" | "keys" | "delete";

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
      <p className="text-sm text-muted" role="status">
        Being deleted: its photos and data are being removed in the background.
      </p>
    );
  }

  return (
    <details className="group">
      <summary
        aria-label={`Manage ${a.email}`}
        className="inline-flex h-8 cursor-pointer list-none items-center gap-1 rounded-full bg-surface px-3 text-[0.8125rem] font-medium select-none marker:hidden hover:bg-line/70 [&::-webkit-details-marker]:hidden"
      >
        Manage
        <ChevronDown className="size-4 text-muted transition-transform duration-200 group-open:rotate-180" aria-hidden="true" />
      </summary>
      <div className="mt-3 flex flex-col gap-3">
        {a.deletion === "failed" && (
          <p className="text-sm text-danger" role="status">
            The last attempt to delete this account failed: {a.deletion_error ?? "unknown error"}. Delete it again to retry.
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          {!mine && (
            <Button
              size="sm"
              disabled={busy}
              onClick={() =>
                void run(
                  () => (disabled ? api.enableAccount(a.id) : api.disableAccount(a.id)),
                  disabled ? "Enabled. It can sign in again and its API keys work." : "Disabled. It is signed out and cannot sign in until you enable it.",
                )
              }
            >
              {disabled ? "Enable account" : "Disable account"}
            </Button>
          )}
          <Button size="sm" disabled={busy} onClick={() => toggle("reset")} aria-expanded={panel === "reset"}>
            Reset password
          </Button>
          <Button
            size="sm"
            disabled={busy}
            onClick={() =>
              void run(
                () => api.setAccountRole(a.id, a.role === "admin" ? "member" : "admin"),
                a.role === "admin" ? "No longer an administrator." : "Now an administrator.",
              )
            }
          >
            {a.role === "admin" ? "Remove admin" : "Make admin"}
          </Button>
          <Button size="sm" disabled={busy} onClick={() => toggle("sessions")} aria-expanded={panel === "sessions"}>
            Sessions
          </Button>
          <Button size="sm" disabled={busy} onClick={() => toggle("keys")} aria-expanded={panel === "keys"}>
            API keys
          </Button>
          {!mine && (
            <Button size="sm" variant="quiet" className="text-danger" disabled={busy} onClick={() => toggle("delete")} aria-expanded={panel === "delete"}>
              Delete account…
            </Button>
          )}
        </div>
        <FormError>{error}</FormError>
        <StatusNote>{note}</StatusNote>
        {panel === "reset" && <ResetPanel account={a} />}
        {panel === "sessions" && <SessionsPanel account={a} />}
        {panel === "keys" && <KeysPanel account={a} onChanged={onChanged} />}
        {panel === "delete" && (
          <DeletePanel
            account={a}
            onDone={async () => {
              setPanel(null);
              await onChanged();
            }}
          />
        )}
      </div>
    </details>
  );
}

function PanelBox({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-3 rounded-card bg-surface p-4 text-sm [overflow-wrap:anywhere]">{children}</div>;
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
    <PanelBox>
      <p className="text-muted">
        Makes a one-time link that lets {a.email} choose a new password, and signs them out everywhere when it is used. It works for 24
        hours. Send it to them yourself; the server sends no email. A new link replaces an older one.
      </p>
      {!link ? (
        <div>
          <Button variant="primary" disabled={busy} onClick={() => void make()}>
            Make a reset link
          </Button>
        </div>
      ) : (
        <SecretBox
          label="Reset link"
          title="Copy this link now. It is not shown again."
          actions={
            <Button variant="primary" onClick={() => void copy()}>
              {copied ? "Copied" : "Copy link"}
            </Button>
          }
        >
          {url}
        </SecretBox>
      )}
      <FormError>{error}</FormError>
    </PanelBox>
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
    <PanelBox>
      {items === null ? (
        !error && <Loading />
      ) : items.length === 0 ? (
        <p>No active sessions.</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {items.map((s) => (
            <li key={s.id}>
              Signed in {when(s.created_at)}
              <span className="text-muted"> · expires {when(s.expires_at)}</span>
            </li>
          ))}
        </ul>
      )}
      {items !== null && items.length > 0 && (
        <div>
          <Button size="sm" disabled={busy} onClick={() => void signOut()}>
            Sign out everywhere
          </Button>
        </div>
      )}
      <FormError>{error}</FormError>
    </PanelBox>
  );
}

/** The account's API keys, the command-line logins among them ("cyanotype CLI on <device>"), with last use and revoke. */
function KeysPanel({ account: a, onChanged }: { account: AdminAccount; onChanged: () => Promise<void> }) {
  const [items, setItems] = useState<AccountApiKey[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(0);
  const [revoking, setRevoking] = useState<AccountApiKey | null>(null);

  useEffect(() => {
    let live = true;
    api
      .accountApiKeys(a.id)
      .then((r) => live && setItems(r.items))
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : "Could not load the API keys."));
    return () => {
      live = false;
    };
  }, [a.id, tick]);

  async function revoke(k: AccountApiKey) {
    setRevoking(null);
    setBusy(true);
    setError(null);
    try {
      await api.revokeAccountApiKey(a.id, k.id);
      setTick((t) => t + 1);
      await onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not revoke the key.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <PanelBox>
      {items === null ? (
        !error && <Loading />
      ) : items.length === 0 ? (
        <p>No API keys.</p>
      ) : (
        <ul className="flex flex-col gap-3">
          {items.map((k) => (
            <li key={k.id} className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <div className="min-w-0 flex-1 basis-48">
                <strong className="font-medium">{k.name}</strong> <Mono>{k.prefix}…</Mono>
                <div className="text-muted">
                  {SCOPE_LABEL[k.scope] ?? k.scope} · created {when(k.created_at)} · last used {k.last_used_at ? when(k.last_used_at) : "never"}
                </div>
              </div>
              <Button size="sm" disabled={busy} aria-label={`Revoke ${k.name}`} onClick={() => setRevoking(k)}>
                Revoke
              </Button>
            </li>
          ))}
        </ul>
      )}
      <FormError>{error}</FormError>
      {revoking && (
        <ConfirmDialog
          title={`Revoke “${revoking.name}”?`}
          confirmLabel="Revoke"
          danger
          onCancel={() => setRevoking(null)}
          onConfirm={() => void revoke(revoking)}
        >
          <p>Anything using it stops working.</p>
        </ConfirmDialog>
      )}
    </PanelBox>
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
    <PanelBox>
      <p>
        <strong>This permanently deletes {a.email}</strong> and everything they stored: every photo and its thumbnails (every version in
        the bucket), albums, share links, API keys and sessions. It cannot be undone. The account is disabled and signed out at once and
        the rest is removed in the background.
      </p>
      <div className="flex flex-col gap-1.5">
        <label htmlFor={id} className="font-medium">
          Type the email address to confirm
        </label>
        <input
          id={id}
          className={inputClass}
          value={typed}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          placeholder={a.email}
          onChange={(e) => setTyped(e.target.value)}
        />
      </div>
      <div>
        <Button variant="danger" disabled={!matches || busy} onClick={() => void remove()}>
          Delete account permanently
        </Button>
      </div>
      <FormError>{error}</FormError>
    </PanelBox>
  );
}
