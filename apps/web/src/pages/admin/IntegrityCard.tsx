import { useCallback, useEffect, useRef, useState } from "react";
import { api, type IntegrityFinding, type IntegrityKind, type IntegritySummary } from "../../api";
import { bytesLabel } from "../../adminText";
import { actedLine, actionsFor, coverageLine, findingTitle, KIND_LABEL, KINDS, runLine, shortHash, type FindingAction } from "../../integrityText";
import { ago } from "../../storageText";

const POLL_MS = 3_000;

/** Admin > Health > Storage integrity: the last run, findings per kind, "Run check now" and the actions on a finding. */
export function IntegrityCard() {
  const [sum, setSum] = useState<IntegritySummary | null>(null);
  const [items, setItems] = useState<IntegrityFinding[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [kind, setKind] = useState<IntegrityKind | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const live = useRef(true);
  const running = !!sum?.task || sum?.last_run?.state === "running";

  const loadList = useCallback(
    async (more: number | null = null) => {
      const page = await api.integrityFindings({ kind, before: more ?? undefined });
      if (!live.current) return;
      setItems((cur) => (more === null ? page.items : [...cur, ...page.items]));
      setNext(page.next_before);
    },
    [kind],
  );
  const load = useCallback(async () => {
    try {
      const s = await api.integrity();
      if (!live.current) return;
      setSum(s);
      setError(null);
    } catch (e) {
      if (live.current) setError(e instanceof Error ? e.message : "Could not load the integrity check.");
    }
  }, []);

  useEffect(() => {
    live.current = true;
    void load();
    return () => {
      live.current = false;
    };
  }, [load]);
  useEffect(() => {
    loadList().catch((e) => live.current && setError(e instanceof Error ? e.message : "Could not load the findings."));
  }, [loadList]);
  // While a check is waiting or running, ask again every few seconds; when it ends, show what it found.
  const wasRunning = useRef(false);
  useEffect(() => {
    if (running) {
      wasRunning.current = true;
      const t = setInterval(() => void load(), POLL_MS);
      return () => clearInterval(t);
    }
    if (wasRunning.current) {
      wasRunning.current = false;
      void loadList().catch(() => {});
    }
  }, [running, load, loadList]);

  async function runNow() {
    setBusy(true);
    setNote(null);
    try {
      await api.runIntegrityCheck();
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not start the check.");
    } finally {
      setBusy(false);
    }
  }

  async function act(f: IntegrityFinding, a: FindingAction, confirm?: "delete") {
    setNote(null);
    try {
      const r = await api.integrityAct(f.id, a.action, confirm);
      setNote(actedLine(a.action, r.changed));
      setError(null);
      await Promise.all([load(), loadList()]);
    } catch (e) {
      setError(e instanceof Error ? e.message : "The action failed.");
    }
  }

  return (
    <section className="health-card" aria-labelledby="integrity-title">
      <h3 id="integrity-title">Storage integrity</h3>
      <p className="muted small">
        Compares the library with the bucket every night: missing originals, objects no photo points at, files that no longer match
        their checksum. Missing thumbnails are made again by themselves.
      </p>
      <div role="alert" className="form-error">
        {error}
      </div>
      {sum && (
        <>
          <p className={`small ${sum.last_run?.state === "failed" ? "health-bad" : ""}`} role="status">
            {runLine(sum.last_run)}
          </p>
          <p className="muted small">{coverageLine(sum.coverage)}</p>
          <div className="invite-actions">
            <button type="button" className="btn btn-quiet" disabled={busy || running} onClick={() => void runNow()}>
              {running ? "Checking…" : "Run check now"}
            </button>
          </div>
          <div className="integrity-counts" role="group" aria-label="Show findings of a kind">
            <button type="button" className={`btn ${kind === undefined ? "btn-primary" : "btn-quiet"}`} aria-pressed={kind === undefined} onClick={() => setKind(undefined)}>
              All ({(KINDS.reduce((n, k) => n + sum.open[k], 0)).toLocaleString("en-US")})
            </button>
            {KINDS.map((k) => (
              <button key={k} type="button" className={`btn ${kind === k ? "btn-primary" : "btn-quiet"}`} aria-pressed={kind === k} onClick={() => setKind(k)}>
                {KIND_LABEL[k]} ({sum.open[k].toLocaleString("en-US")})
              </button>
            ))}
          </div>
          {sum.acknowledged > 0 && <p className="muted small">{sum.acknowledged.toLocaleString("en-US")} marked lost or left as they are.</p>}
        </>
      )}
      {items.length === 0 ? (
        sum && <p className="small">No open findings.</p>
      ) : (
        <ul className="integrity-list" aria-label="Findings">
          {items.map((f) => (
            <Finding key={f.id} f={f} onAct={act} />
          ))}
        </ul>
      )}
      {next !== null && (
        <button type="button" className="btn btn-quiet" onClick={() => void loadList(next)}>
          Show more
        </button>
      )}
      <p className="small" role="status">
        {note}
      </p>
    </section>
  );
}

function Finding({ f, onAct }: { f: IntegrityFinding; onAct: (f: IntegrityFinding, a: FindingAction, confirm?: "delete") => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<FindingAction | null>(null);
  const [typed, setTyped] = useState("");
  const d = f.detail;

  async function go(a: FindingAction, confirm?: "delete") {
    setBusy(true);
    try {
      await onAct(f, a, confirm);
    } finally {
      setBusy(false);
      setConfirming(null);
      setTyped("");
    }
  }

  return (
    <li>
      <strong>{findingTitle(f)}</strong>
      <code className="integrity-key">{f.key}</code>
      <span className="muted small">
        {f.owner_email ?? "no such account"} · seen {ago(f.last_seen_at)}
      </span>
      {f.kind === "checksum_mismatch" && d.reason === "hash" && (
        <div className="small">
          Stored <code>{shortHash(d.expected_sha256)}</code>, found <code>{shortHash(d.actual_sha256)}</code>
        </div>
      )}
      {f.kind === "untracked_object" && d.size_bytes != null && <div className="muted small">{bytesLabel(d.size_bytes)}</div>}
      {f.kind !== "untracked_object" && d.previous_versions === 0 && <div className="muted small">The bucket holds no earlier version.</div>}
      <div className="invite-actions">
        {actionsFor(f).map((a) => (
          <button
            key={a.action}
            type="button"
            className={`btn ${a.danger ? "btn-danger" : "btn-quiet"}`}
            disabled={busy}
            onClick={() => (a.danger ? setConfirming(a) : void go(a))}
          >
            {a.label}
          </button>
        ))}
      </div>
      {confirming && (
        <div className="integrity-confirm" role="group" aria-label="Confirm deleting the object">
          <p className="small">This deletes every version of the object for good. Type delete to confirm.</p>
          <input aria-label="Type delete to confirm" value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" />
          <div className="invite-actions">
            <button type="button" className="btn btn-quiet" onClick={() => { setConfirming(null); setTyped(""); }}>
              Cancel
            </button>
            <button type="button" className="btn btn-danger" disabled={busy || typed.trim().toLowerCase() !== "delete"} onClick={() => void go(confirming, "delete")}>
              Delete for good
            </button>
          </div>
        </div>
      )}
    </li>
  );
}
