import { useCallback, useEffect, useRef, useState } from "react";
import { api, type IntegrityFinding, type IntegrityKind, type IntegritySummary } from "../../api";
import { bytesLabel } from "../../adminText";
import { actedLine, actionsFor, coverageLine, findingTitle, KIND_LABEL, KINDS, runLine, shortHash, type FindingAction } from "../../integrityText";
import { ago } from "../../storageText";
import { Card, FormError, Mono, inputClass } from "../../settings/kit";
import { Button } from "../../ui/Button";
import { cn } from "../../ui/cn";

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
    <Card id="integrity-title" title="Storage integrity">
      <p className="text-muted">
        Compares the library with the bucket every night: missing originals, objects no photo points at, files that no longer match
        their checksum. Missing thumbnails are made again by themselves.
      </p>
      <FormError>{error}</FormError>
      {sum && (
        <>
          <p className={cn(sum.last_run?.state === "failed" && "text-danger")} role="status">
            {runLine(sum.last_run)}
          </p>
          <p className="text-[0.8125rem] text-muted">{coverageLine(sum.coverage)}</p>
          <div>
            <Button size="sm" disabled={busy || running} onClick={() => void runNow()}>
              {running ? "Checking…" : "Run check now"}
            </Button>
          </div>
          <div className="flex flex-wrap gap-2" role="group" aria-label="Show findings of a kind">
            <Button size="sm" variant={kind === undefined ? "primary" : "secondary"} aria-pressed={kind === undefined} onClick={() => setKind(undefined)}>
              All ({KINDS.reduce((n, k) => n + sum.open[k], 0).toLocaleString("en-US")})
            </Button>
            {KINDS.map((k) => (
              <Button key={k} size="sm" variant={kind === k ? "primary" : "secondary"} aria-pressed={kind === k} onClick={() => setKind(k)}>
                {KIND_LABEL[k]} ({sum.open[k].toLocaleString("en-US")})
              </Button>
            ))}
          </div>
          {sum.acknowledged > 0 && <p className="text-[0.8125rem] text-muted">{sum.acknowledged.toLocaleString("en-US")} marked lost or left as they are.</p>}
        </>
      )}
      {items.length === 0 ? (
        sum && <p>No open findings.</p>
      ) : (
        <ul className="divide-y divide-line rounded-xl bg-surface" aria-label="Findings">
          {items.map((f) => (
            <Finding key={f.id} f={f} onAct={act} />
          ))}
        </ul>
      )}
      {next !== null && (
        <div>
          <Button size="sm" onClick={() => void loadList(next)}>
            Show more
          </Button>
        </div>
      )}
      <p role="status" className="empty:hidden">
        {note}
      </p>
    </Card>
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
    <li className="flex flex-col gap-1.5 p-3.5">
      <strong className="font-medium">{findingTitle(f)}</strong>
      <code className="font-mono text-[0.8125rem] break-all text-muted">{f.key}</code>
      <span className="text-[0.8125rem] break-words text-muted">
        {f.owner_email ?? "no such account"} · seen {ago(f.last_seen_at)}
      </span>
      {f.kind === "checksum_mismatch" && d.reason === "hash" && (
        <div>
          Stored <Mono>{shortHash(d.expected_sha256)}</Mono>, found <Mono>{shortHash(d.actual_sha256)}</Mono>
        </div>
      )}
      {f.kind === "untracked_object" && d.size_bytes != null && <div className="text-[0.8125rem] text-muted">{bytesLabel(d.size_bytes)}</div>}
      {f.kind !== "untracked_object" && d.previous_versions === 0 && <div className="text-[0.8125rem] text-muted">The bucket holds no earlier version.</div>}
      <div className="mt-1 flex flex-wrap gap-2">
        {actionsFor(f).map((a) => (
          <Button
            key={a.action}
            size="sm"
            className="h-auto min-h-8 py-1.5 whitespace-normal"
            variant={a.danger ? "danger" : "secondary"}
            disabled={busy}
            onClick={() => (a.danger ? setConfirming(a) : void go(a))}
          >
            {a.label}
          </Button>
        ))}
      </div>
      {confirming && (
        <div className="mt-2 flex flex-col gap-3 rounded-xl bg-card p-3 ring-1 ring-line" role="group" aria-label="Confirm deleting the object">
          <p>This deletes every version of the object for good. Type delete to confirm.</p>
          <input
            className={inputClass}
            aria-label="Type delete to confirm"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            autoComplete="off"
          />
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="quiet"
              onClick={() => {
                setConfirming(null);
                setTyped("");
              }}
            >
              Cancel
            </Button>
            <Button size="sm" variant="danger" disabled={busy || typed.trim().toLowerCase() !== "delete"} onClick={() => void go(confirming, "delete")}>
              Delete for good
            </Button>
          </div>
        </div>
      )}
    </li>
  );
}
