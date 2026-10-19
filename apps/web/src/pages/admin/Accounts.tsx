import { useCallback, useEffect, useRef, useState } from "react";
import { api, type AdminAccount, type AdminAccounts, type AdminTask, type BucketReport } from "../../api";
import { bytesLabel } from "../../adminText";
import { ago, bucketTaskLine, countLabel, derivedLabel, mismatchLine, reconcileLine } from "../../storageText";

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
    <section aria-labelledby="accounts-title">
      <h2 id="accounts-title">Accounts</h2>
      <p className="muted">Counts and sizes only. Nobody&apos;s photos or file names are shown here.</p>
      <div role="alert" className="form-error">
        {error}
      </div>
      {data === null ? (
        !error && (
          <p className="muted" role="status">
            Loading…
          </p>
        )
      ) : (
        <>
          <Totals totals={data.totals} />
          <p className="muted small">{reconcileLine(data.reconcile)}</p>
          <ul className="account-list">
            {data.items.map((a) => (
              <AccountCard key={a.id} a={a} />
            ))}
          </ul>
          <BucketCheck task={task} busy={starting || open} onStart={() => void startCheck()} />
        </>
      )}
    </section>
  );
}

function Totals({ totals }: { totals: AdminAccounts["totals"] }) {
  return (
    <dl className="stat-grid" aria-label="Server totals">
      <Stat label="Accounts" value={totals.accounts.toLocaleString("en-US")} />
      <Stat label="Photos" value={totals.photo_count.toLocaleString("en-US")} sub={`${countLabel(totals.raw_count, "RAW file")}`} />
      <Stat label="Originals" value={bytesLabel(totals.originals_bytes)} />
      <Stat label="Trash" value={bytesLabel(totals.trash_bytes)} />
      <Stat label="Thumbnails" value={derivedLabel(totals.derived_bytes, totals.derived_complete)} />
      <Stat
        label="Uploaded, 24 h"
        value={bytesLabel(totals.uploads_24h_bytes)}
        sub={countLabel(totals.uploads_24h_count, "file")}
      />
      <Stat
        label="Uploaded, 30 d"
        value={bytesLabel(totals.uploads_30d_bytes)}
        sub={countLabel(totals.uploads_30d_count, "file")}
      />
    </dl>
  );
}

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="stat">
      <dt>{label}</dt>
      <dd>
        {value}
        {sub && <span className="muted small"> {sub}</span>}
      </dd>
    </div>
  );
}

function AccountCard({ a }: { a: AdminAccount }) {
  return (
    <li className="account">
      <div className="account-head">
        <strong className="account-email">{a.email}</strong>
        <span className={`badge badge-role-${a.role}`}>{a.role}</span>
      </div>
      <div className="muted small">
        Joined {new Date(a.created_at).toLocaleDateString()} · last active {ago(a.last_active)}
      </div>
      <dl className="account-stats">
        <Stat label="Photos" value={a.photo_count.toLocaleString("en-US")} sub={a.raw_count ? `${a.raw_count.toLocaleString("en-US")} RAW` : undefined} />
        <Stat label="Storage" value={bytesLabel(a.originals_bytes)} />
        <Stat label="Trash" value={bytesLabel(a.trash_bytes)} sub={a.trash_count ? countLabel(a.trash_count, "file") : undefined} />
        <Stat label="Uploads, 24 h" value={bytesLabel(a.uploads_24h_bytes)} sub={countLabel(a.uploads_24h_count, "file")} />
        <Stat label="Uploads, 30 d" value={bytesLabel(a.uploads_30d_bytes)} sub={countLabel(a.uploads_30d_count, "file")} />
        <Stat label="Albums" value={a.albums.toLocaleString("en-US")} />
        <Stat label="Shares" value={a.shares.toLocaleString("en-US")} />
        <Stat label="API keys" value={a.api_keys.toLocaleString("en-US")} />
      </dl>
    </li>
  );
}

function BucketCheck({ task, busy, onStart }: { task: AdminTask<BucketReport> | null; busy: boolean; onStart: () => void }) {
  const r = task?.state === "done" ? task.result : null;
  return (
    <section className="bucket-check" aria-labelledby="bucket-title">
      <h3 id="bucket-title">Bucket check</h3>
      <p className="muted small">
        Lists every object in the bucket and compares it with the database. It is slow on a big library and costs a request per
        thousand objects, so it runs in the background and only when you ask. Run it when nobody is uploading.
      </p>
      <button type="button" className="btn btn-quiet" disabled={busy} onClick={onStart}>
        {busy ? "Checking…" : "Check bucket size"}
      </button>
      <p className="small" role="status">
        {bucketTaskLine(task)}
      </p>
      {r && (
        <>
          <dl className="stat-grid">
            <Stat label="Originals in the bucket" value={bytesLabel(r.originals_bytes)} sub={`database ${bytesLabel(r.db_originals_bytes)}`} />
            <Stat label="Thumbnails in the bucket" value={bytesLabel(r.derived_bytes)} sub={`measured ${bytesLabel(r.db_derived_bytes)}`} />
            <Stat label="Old versions kept" value={bytesLabel(r.noncurrent_bytes)} sub={countLabel(r.noncurrent_versions, "version")} />
            <Stat label="Everything else" value={bytesLabel(r.manifests_bytes + r.edits_bytes + r.other_bytes)} sub="manifests, edits, backups" />
            <Stat label="Total, current" value={bytesLabel(r.current_bytes)} sub={countLabel(r.objects, "object")} />
          </dl>
          {r.mismatched.length === 0 && r.unknown_account_bytes === 0 ? (
            <p className="small">Originals in the bucket match the database for every account.</p>
          ) : (
            <div role="group" aria-label="Differences">
              <p className="small">
                <strong>Differences found.</strong> A photo uploaded or deleted while the check ran can show up here once; run it
                again before acting on it.
              </p>
              <ul className="mismatch-list small">
                {r.mismatched.map((m) => (
                  <li key={m.user_id}>{mismatchLine(m)}</li>
                ))}
                {r.unknown_account_bytes > 0 && <li>{bytesLabel(r.unknown_account_bytes)} belong to an account the database does not know.</li>}
              </ul>
            </div>
          )}
          {!r.versioned && <p className="muted small">This store did not list old versions, so none are counted.</p>}
        </>
      )}
    </section>
  );
}
