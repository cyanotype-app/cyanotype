import { useCallback, useEffect, useRef, useState } from "react";
import { api, type AdminAccount, type AdminAccounts, type AdminTask, type BucketReport, type Limits } from "../../api";
import { AccountActions } from "../../components/AccountActions";
import { useAuthOptional } from "../../auth";
import { LimitsForm } from "../../components/LimitsForm";
import { levelOf, limitsWords, percentOf } from "../../limitsText";
import { bytesLabel } from "../../adminText";
import { ago, bucketTaskLine, countLabel, derivedLabel, mismatchLine, reconcileLine } from "../../storageText";
import { FormError, Loading, Section } from "../../settings/kit";
import { Button } from "../../ui/Button";
import { cn } from "../../ui/cn";
import { useDesktop } from "../../ui/useMedia";

/** Admin > Accounts: every account's counts and sizes (never their photos), the server totals and the bucket check. */
export function Accounts() {
  const [data, setData] = useState<AdminAccounts | null>(null);
  const [task, setTask] = useState<AdminTask<BucketReport> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const live = useRef(true);

  const load = useCallback(async () => {
    try {
      const d = await api.adminAccounts();
      if (!live.current) return;
      setData(d);
      setTask(d.bucket_check);
    } catch (e) {
      if (live.current) setError(e instanceof Error ? e.message : "Could not load the accounts.");
    }
  }, []);
  useEffect(() => {
    live.current = true;
    void load();
    return () => {
      live.current = false;
    };
  }, [load]);

  // While a bucket check is waiting or running, ask again every few seconds.
  const open = task?.state === "queued" || task?.state === "running";
  useEffect(() => {
    if (!open) return;
    const t = setInterval(() => {
      api
        .bucketCheck()
        .then((r) => live.current && setTask(r.task))
        .catch(() => undefined);
    }, 3000);
    return () => clearInterval(t);
  }, [open]);

  async function startCheck() {
    setStarting(true);
    setError(null);
    try {
      setTask((await api.startBucketCheck()).task);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not start the check.");
    } finally {
      setStarting(false);
    }
  }

  return (
    <section aria-label="Accounts" className="[overflow-wrap:anywhere]">
      <p className="text-sm text-muted">Counts and sizes only. Nobody&apos;s photos or file names are shown here.</p>
      <div className="mt-3">
        <FormError>{error}</FormError>
      </div>
      {data === null ? (
        !error && <Loading />
      ) : (
        <>
          <Totals totals={data.totals} />
          <p className="mt-2 text-[0.8125rem] text-muted">{reconcileLine(data.reconcile)}</p>
          <DefaultLimits
            limits={data.default_limits ?? { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null }}
            onSaved={() => load()}
          />
          <AccountList items={data.items} onSaved={() => load()} />
          <BucketCheck task={task} busy={starting || open} onStart={() => void startCheck()} />
        </>
      )}
    </section>
  );
}

function Totals({ totals }: { totals: AdminAccounts["totals"] }) {
  return (
    <dl className="mt-4 grid grid-cols-2 gap-2 min-[560px]:grid-cols-3 lg:grid-cols-4" aria-label="Server totals">
      <Stat tile label="Accounts" value={totals.accounts.toLocaleString("en-US")} />
      <Stat tile label="Photos" value={totals.photo_count.toLocaleString("en-US")} sub={`${countLabel(totals.raw_count, "RAW file")}`} />
      <Stat tile label="Originals" value={bytesLabel(totals.originals_bytes)} />
      <Stat tile label="Recently deleted" value={bytesLabel(totals.trash_bytes)} />
      <Stat tile label="Thumbnails" value={derivedLabel(totals.derived_bytes, totals.derived_complete)} />
      <Stat tile label="Uploaded, 24 h" value={bytesLabel(totals.uploads_24h_bytes)} sub={countLabel(totals.uploads_24h_count, "file")} />
      <Stat tile label="Uploaded, 30 d" value={bytesLabel(totals.uploads_30d_bytes)} sub={countLabel(totals.uploads_30d_count, "file")} />
    </dl>
  );
}

function Stat({ label, value, sub, tile }: { label: string; value: string; sub?: string; tile?: boolean }) {
  return (
    <div className={cn("min-w-0", tile && "rounded-card bg-card p-3 ring-1 ring-line")}>
      <dt className="text-xs text-muted">{label}</dt>
      <dd className={cn("mt-0.5 break-words", tile ? "text-[1.0625rem] font-semibold tracking-tight" : "text-[0.9375rem]")}>
        {value}
        {sub && <span className="text-[0.8125rem] font-normal text-muted"> {sub}</span>}
      </dd>
    </div>
  );
}

function DefaultLimits({ limits, onSaved }: { limits: Limits; onSaved: () => Promise<void> }) {
  const [editing, setEditing] = useState(false);
  return (
    <Section
      title="Limits for new accounts"
      id="default-limits-title"
      description="Accounts created from now on start with these (an invite can preset its own). Existing accounts are not changed, and the first account, the administrator, starts without limits."
    >
      <div className="rounded-card bg-card p-4 ring-1 ring-line">
        {editing ? (
          <LimitsForm
            initial={limits}
            saveLabel="Save defaults"
            onCancel={() => setEditing(false)}
            onSave={async (l) => {
              await api.setDefaultLimits(l);
              setEditing(false);
              await onSaved();
            }}
          />
        ) : (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <p className="min-w-0 flex-1 basis-48 text-[0.9375rem]">{limitsWords(limits)}</p>
            <Button size="sm" onClick={() => setEditing(true)}>
              Edit defaults
            </Button>
          </div>
        )}
      </div>
    </Section>
  );
}

function useMe(): string | undefined {
  const me = useAuthOptional();
  return me?.state.status === "authenticated" ? me.state.user.id : undefined;
}

const limitsOf = (a: AdminAccount): Limits => ({
  quota_bytes: a.quota_bytes ?? null,
  daily_upload_bytes: a.daily_upload_bytes ?? null,
  max_file_bytes: a.max_file_bytes ?? null,
});

/** Cards on a phone, a table on a desktop. */
function AccountList({ items, onSaved }: { items: AdminAccount[]; onSaved: () => Promise<void> }) {
  const desktop = useDesktop();
  const meId = useMe();
  return (
    <Section title="All accounts">
      {desktop ? (
        <div className="overflow-hidden rounded-card bg-card ring-1 ring-line">
          <table className="w-full table-fixed border-collapse text-left text-[0.875rem]">
            <thead>
              <tr className="text-xs text-muted">
                <th scope="col" className="w-[34%] px-4 py-2.5 font-medium">Account</th>
                <th scope="col" className="px-2 py-2.5 font-medium">Photos</th>
                <th scope="col" className="px-2 py-2.5 font-medium">Storage</th>
                <th scope="col" className="px-2 py-2.5 font-medium">Deleted</th>
                <th scope="col" className="px-2 py-2.5 font-medium">24 h</th>
                <th scope="col" className="px-4 py-2.5 font-medium">30 d</th>
              </tr>
            </thead>
            {items.map((a) => (
              <AccountRow key={a.id} a={a} meId={meId} onSaved={onSaved} />
            ))}
          </table>
        </div>
      ) : (
        <ul aria-label="Accounts" className="flex flex-col gap-3">
          {items.map((a) => (
            <AccountCard key={a.id} a={a} meId={meId} onSaved={onSaved} />
          ))}
        </ul>
      )}
    </Section>
  );
}

function Badges({ a, meId }: { a: AdminAccount; meId?: string }) {
  const chip = "inline-flex h-5 items-center rounded-full bg-surface px-2 text-xs font-medium";
  return (
    <>
      <span className={cn(chip, a.role === "admin" && "bg-accent/12 text-accent")}>{a.role}</span>
      {a.disabled_at && <span className={cn(chip, "text-danger")}>disabled</span>}
      {a.deletion && <span className={cn(chip, "text-danger")}>deleting</span>}
      {meId === a.id && <span className={chip}>you</span>}
    </>
  );
}

function AccountLimits({ a, onSaved }: { a: AdminAccount; onSaved: () => Promise<void> }) {
  const [editing, setEditing] = useState(false);
  const limits = limitsOf(a);
  const stored = a.originals_bytes + a.trash_bytes;
  const level = limits.quota_bytes ? levelOf(stored, limits.quota_bytes) : "ok";
  if (editing) {
    return (
      <LimitsForm
        initial={limits}
        onCancel={() => setEditing(false)}
        onSave={async (l) => {
          await api.setAccountLimits(a.id, l);
          setEditing(false);
          await onSaved();
        }}
      />
    );
  }
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <span className={cn("min-w-0 flex-1 basis-40 text-sm", level === "ok" ? "text-muted" : "text-danger")}>
        {limitsWords(limits)}
        {limits.quota_bytes ? ` · ${percentOf(stored, limits.quota_bytes)}% of storage used` : ""}
      </span>
      <Button size="sm" aria-label={`Edit limits of ${a.email}`} onClick={() => setEditing(true)}>
        Edit limits
      </Button>
    </div>
  );
}

function AccountCard({ a, meId, onSaved }: { a: AdminAccount; meId?: string; onSaved: () => Promise<void> }) {
  return (
    <li className="rounded-card bg-card p-4 ring-1 ring-line">
      <div className="flex flex-wrap items-center gap-2">
        <strong className="min-w-0 break-all text-[0.9375rem] font-semibold">{a.email}</strong>
        <Badges a={a} meId={meId} />
      </div>
      <div className="mt-0.5 text-[0.8125rem] text-muted">
        Joined {new Date(a.created_at).toLocaleDateString()} · last active {ago(a.last_active)}
      </div>
      <dl className="mt-3 grid grid-cols-2 gap-x-4 gap-y-2.5">
        <Stat label="Photos" value={a.photo_count.toLocaleString("en-US")} sub={a.raw_count ? `${a.raw_count.toLocaleString("en-US")} RAW` : undefined} />
        <Stat label="Storage" value={bytesLabel(a.originals_bytes)} />
        <Stat label="Recently deleted" value={bytesLabel(a.trash_bytes)} sub={a.trash_count ? countLabel(a.trash_count, "file") : undefined} />
        <Stat label="Uploads, 24 h" value={bytesLabel(a.uploads_24h_bytes)} sub={countLabel(a.uploads_24h_count, "file")} />
        <Stat label="Uploads, 30 d" value={bytesLabel(a.uploads_30d_bytes)} sub={countLabel(a.uploads_30d_count, "file")} />
        <Stat label="Albums" value={a.albums.toLocaleString("en-US")} />
        <Stat label="Shares" value={a.shares.toLocaleString("en-US")} />
        <Stat label="API keys" value={a.api_keys.toLocaleString("en-US")} />
      </dl>
      <div className="mt-4 border-t border-line pt-3">
        <AccountActions account={a} meId={meId} onChanged={onSaved} />
      </div>
      <div className="mt-3 border-t border-line pt-3">
        <AccountLimits a={a} onSaved={onSaved} />
      </div>
    </li>
  );
}

function AccountRow({ a, meId, onSaved }: { a: AdminAccount; meId?: string; onSaved: () => Promise<void> }) {
  const num = "px-2 py-3 align-top tabular-nums";
  return (
    <tbody className="border-t border-line">
      <tr>
        <th scope="row" className="px-4 py-3 text-left align-top font-normal">
          <div className="flex flex-wrap items-center gap-1.5">
            <strong className="min-w-0 break-all font-semibold">{a.email}</strong>
            <Badges a={a} meId={meId} />
          </div>
          <div className="mt-0.5 text-xs text-muted">
            Joined {new Date(a.created_at).toLocaleDateString()} · last active {ago(a.last_active)}
          </div>
        </th>
        <td className={num}>
          {a.photo_count.toLocaleString("en-US")}
          {a.raw_count > 0 && <div className="text-xs text-muted">{a.raw_count.toLocaleString("en-US")} RAW</div>}
        </td>
        <td className={num}>{bytesLabel(a.originals_bytes)}</td>
        <td className={num}>
          {bytesLabel(a.trash_bytes)}
          {a.trash_count > 0 && <div className="text-xs text-muted">{countLabel(a.trash_count, "file")}</div>}
        </td>
        <td className={num}>
          {bytesLabel(a.uploads_24h_bytes)}
          <div className="text-xs text-muted">{countLabel(a.uploads_24h_count, "file")}</div>
        </td>
        <td className={cn(num, "px-4")}>
          {bytesLabel(a.uploads_30d_bytes)}
          <div className="text-xs text-muted">{countLabel(a.uploads_30d_count, "file")}</div>
        </td>
      </tr>
      <tr>
        <td colSpan={6} className="px-4 pb-4">
          <div className="flex flex-col gap-3">
            <p className="text-xs text-muted">
              {a.albums.toLocaleString("en-US")} albums · {a.shares.toLocaleString("en-US")} shares · {a.api_keys.toLocaleString("en-US")} API keys
            </p>
            <AccountActions account={a} meId={meId} onChanged={onSaved} />
            <AccountLimits a={a} onSaved={onSaved} />
          </div>
        </td>
      </tr>
    </tbody>
  );
}

function BucketCheck({ task, busy, onStart }: { task: AdminTask<BucketReport> | null; busy: boolean; onStart: () => void }) {
  const r = task?.state === "done" ? task.result : null;
  return (
    <Section
      title="Bucket check"
      id="bucket-title"
      description="Lists every object in the bucket and compares it with the database. It is slow on a big library and costs a request per thousand objects, so it runs in the background and only when you ask. Run it when nobody is uploading."
    >
      <div className="flex flex-col gap-3">
        <div>
          <Button disabled={busy} onClick={onStart}>
            {busy ? "Checking…" : "Check bucket size"}
          </Button>
        </div>
        <p className="text-sm" role="status">
          {bucketTaskLine(task)}
        </p>
        {r && (
          <>
            <dl className="grid grid-cols-2 gap-2 min-[560px]:grid-cols-3">
              <Stat tile label="Originals in the bucket" value={bytesLabel(r.originals_bytes)} sub={`database ${bytesLabel(r.db_originals_bytes)}`} />
              <Stat tile label="Thumbnails in the bucket" value={bytesLabel(r.derived_bytes)} sub={`measured ${bytesLabel(r.db_derived_bytes)}`} />
              <Stat tile label="Old versions kept" value={bytesLabel(r.noncurrent_bytes)} sub={countLabel(r.noncurrent_versions, "version")} />
              <Stat tile label="Everything else" value={bytesLabel(r.manifests_bytes + r.edits_bytes + r.other_bytes)} sub="manifests, edits, backups" />
              <Stat tile label="Total, current" value={bytesLabel(r.current_bytes)} sub={countLabel(r.objects, "object")} />
            </dl>
            {r.mismatched.length === 0 && r.unknown_account_bytes === 0 ? (
              <p className="text-sm">Originals in the bucket match the database for every account.</p>
            ) : (
              <div role="group" aria-label="Differences" className="rounded-card bg-surface p-4 text-sm">
                <p>
                  <strong>Differences found.</strong> A photo uploaded or deleted while the check ran can show up here once; run it again before
                  acting on it.
                </p>
                <ul className="mt-2 flex list-disc flex-col gap-1 pl-5">
                  {r.mismatched.map((m) => (
                    <li key={m.user_id}>{mismatchLine(m)}</li>
                  ))}
                  {r.unknown_account_bytes > 0 && <li>{bytesLabel(r.unknown_account_bytes)} belong to an account the database does not know.</li>}
                </ul>
              </div>
            )}
            {!r.versioned && <p className="text-[0.8125rem] text-muted">This store did not list old versions, so none are counted.</p>}
          </>
        )}
      </div>
    </Section>
  );
}
