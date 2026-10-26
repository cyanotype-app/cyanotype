import { ChevronDown } from "lucide-react";
import { useCallback, useEffect, useState, type FormEvent } from "react";
import { api, type Invite, type IssuedInvite, type NewInvite } from "../../api";
import { gigabytesToBytes, inviteStatusLine, inviteUrl, limitsLine } from "../../adminText";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { Field, FormError, Group, inputClass, Loading, Row, Section, SecretBox, SelectBox } from "../../settings/kit";
import { Button } from "../../ui/Button";
import { cn } from "../../ui/cn";

const when = (iso: string) => new Date(iso).toLocaleString();
const EXPIRY_CHOICES = [1, 3, 7, 14, 30, 90];

type Ask = { kind: "reissue"; invite: Invite } | { kind: "revoke"; invite: Invite };

/** Create, list, re-issue (show the link again) and revoke registration invites. */
export function Invites() {
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
  const [ask, setAsk] = useState<Ask | null>(null);

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
    setAsk(null);
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
    setAsk(null);
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

  const limitField = (label: string, value: string, set: (v: string) => void) => (
    <Field label={label}>
      {(p) => <input {...p} className={inputClass} inputMode="decimal" value={value} placeholder="No limit" onChange={(e) => set(e.target.value)} />}
    </Field>
  );

  return (
    <section aria-label="Invites" className="[overflow-wrap:anywhere]">
      <p className="text-sm text-muted">
        An invite is a single-use link that lets one person create an account. Send it to them yourself; the server sends no email.
      </p>

      <Section title="New invite">
        <form className="flex flex-col gap-4 rounded-card bg-card p-4 ring-1 ring-line" onSubmit={create}>
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-[minmax(0,1fr)_11rem]">
            <Field label="Note (only you see it)">
              {(p) => <input {...p} className={inputClass} value={note} maxLength={200} placeholder="e.g. for Sam" onChange={(e) => setNote(e.target.value)} />}
            </Field>
            <Field label="Expires after">
              {(p) => (
                <SelectBox {...p} value={days} onChange={(e) => setDays(Number(e.target.value))}>
                  {EXPIRY_CHOICES.map((n) => (
                    <option key={n} value={n}>
                      {n} {n === 1 ? "day" : "days"}
                    </option>
                  ))}
                </SelectBox>
              )}
            </Field>
          </div>
          <details className="group">
            <summary className="inline-flex cursor-pointer list-none items-center gap-1 text-sm font-medium text-accent select-none [&::-webkit-details-marker]:hidden">
              Starting limits (optional)
              <ChevronDown className="size-4 transition-transform duration-200 group-open:rotate-180" aria-hidden="true" />
            </summary>
            <p className="mt-2 text-[0.8125rem] text-muted">Copied onto the new account when the invite is used. Empty means no limit.</p>
            <div className="mt-3 grid grid-cols-1 gap-3 min-[480px]:grid-cols-3">
              {limitField("Storage (GB)", quota, setQuota)}
              {limitField("Uploads per day (GB)", daily, setDaily)}
              {limitField("Largest file (GB)", maxFile, setMaxFile)}
            </div>
          </details>
          <FormError>{error}</FormError>
          <div>
            <Button type="submit" variant="primary" disabled={busy}>
              Create invite
            </Button>
          </div>
        </form>
      </Section>

      {issued && (
        <Section>
          <SecretBox
            label="Invite link"
            title="Copy this link now and send it. It is not shown again."
            actions={
              <>
                <Button variant="primary" onClick={() => void copy()}>
                  {copied ? "Copied" : "Copy link"}
                </Button>
                <Button variant="quiet" onClick={() => setIssued(null)}>
                  Done
                </Button>
              </>
            }
          >
            {link}
          </SecretBox>
          <p className="mt-2 text-[0.8125rem] text-muted">
            It works once and expires {new Date(issued.invite.expires_at).toLocaleDateString()}. If you lose it, make a new link from the list.
          </p>
        </Section>
      )}

      <Section title="Links">
        {items === null ? (
          <Loading />
        ) : items.length === 0 ? (
          <p className="text-sm text-muted">No invites yet.</p>
        ) : (
          <Group as="ul">
            {items.map((inv) => {
              const limits = limitsLine(inv.limits);
              return (
                <Row
                  as="li"
                  key={inv.id}
                  className="py-3"
                  label={
                    <span className="flex flex-wrap items-center gap-2">
                      <strong className="font-medium">{inv.note ?? "No note"}</strong>
                      <span
                        className={cn(
                          "inline-flex h-5 items-center rounded-full bg-surface px-2 text-xs font-medium",
                          inv.status === "pending" && "bg-accent/12 text-accent",
                          (inv.status === "revoked" || inv.status === "expired") && "text-muted",
                        )}
                      >
                        {inv.status}
                      </span>
                    </span>
                  }
                  detail={
                    <>
                      <span className="block">{inviteStatusLine(inv)}</span>
                      <span className="block">
                        Created {when(inv.created_at)}
                        {inv.created_by ? ` by ${inv.created_by}` : ""}
                        {inv.used_at ? ` · used ${when(inv.used_at)}` : ""}
                      </span>
                      {limits && <span className="block">Starts with {limits}</span>}
                    </>
                  }
                >
                  {(inv.status === "pending" || inv.status === "expired") && (
                    <>
                      <Button size="sm" onClick={() => setAsk({ kind: "reissue", invite: inv })}>
                        {inv.status === "expired" ? "Renew link" : "Show link again"}
                      </Button>
                      {inv.status === "pending" && (
                        <Button size="sm" onClick={() => setAsk({ kind: "revoke", invite: inv })}>
                          Revoke
                        </Button>
                      )}
                    </>
                  )}
                </Row>
              );
            })}
          </Group>
        )}
      </Section>

      {ask?.kind === "reissue" && (
        <ConfirmDialog
          title={ask.invite.status === "expired" ? "Renew the link?" : "Make a new link?"}
          confirmLabel="Make a new link"
          onCancel={() => setAsk(null)}
          onConfirm={() => void showLinkAgain(ask.invite)}
        >
          <p>A new link replaces the old one: the old link stops working.</p>
        </ConfirmDialog>
      )}
      {ask?.kind === "revoke" && (
        <ConfirmDialog title="Revoke this invite?" confirmLabel="Revoke" danger onCancel={() => setAsk(null)} onConfirm={() => void revoke(ask.invite)}>
          <p>Its link stops working.</p>
        </ConfirmDialog>
      )}
    </section>
  );
}
