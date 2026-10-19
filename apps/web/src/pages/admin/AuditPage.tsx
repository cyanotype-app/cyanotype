import { useCallback, useEffect, useId, useRef, useState, type FormEvent } from "react";
import { api, type AuditEntry, type AuditQuery } from "../../api";
import { actionLabel, detailLines } from "../../auditText";

const when = (iso: string) => new Date(iso).toLocaleString();

interface Filters {
  action: string;
  actor: string;
  target: string;
  from: string;
  to: string;
}
const NONE: Filters = { action: "", actor: "", target: "", from: "", to: "" };

/** Admin > Audit log: what admins (and invite use) did, newest first, filterable by action, who, what and when. */
export function AuditPage() {
  const id = useId();
  const [form, setForm] = useState<Filters>(NONE);
  const [applied, setApplied] = useState<Filters>(NONE);
  const [items, setItems] = useState<AuditEntry[] | null>(null);
  const [actions, setActions] = useState<string[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const live = useRef(true);

  const query = (f: Filters, before?: number): AuditQuery => ({ ...f, before, limit: 50 });

  const load = useCallback(async (f: Filters) => {
    setBusy(true);
    setError(null);
    try {
      const page = await api.auditLog(query(f));
      if (!live.current) return;
      setItems(page.items);
      setNext(page.next_before);
      setActions(page.actions);
    } catch (e) {
      if (live.current) setError(e instanceof Error ? e.message : "Could not load the audit log.");
    } finally {
      if (live.current) setBusy(false);
    }
  }, []);
  useEffect(() => {
    live.current = true;
    void load(NONE);
    return () => {
      live.current = false;
    };
  }, [load]);

  async function more() {
    if (next === null) return;
    setBusy(true);
    try {
      const page = await api.auditLog(query(applied, next));
      setItems((cur) => [...(cur ?? []), ...page.items]);
      setNext(page.next_before);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load more.");
    } finally {
      setBusy(false);
    }
  }

  function apply(e: FormEvent) {
    e.preventDefault();
    setApplied(form);
    void load(form);
  }
  function clear() {
    setForm(NONE);
    setApplied(NONE);
    void load(NONE);
  }
  const set = (k: keyof Filters) => (e: { target: { value: string } }) => setForm((f) => ({ ...f, [k]: e.target.value }));

  return (
    <section aria-labelledby="audit-title">
      <h2 id="audit-title">Audit log</h2>
      <p className="muted">Administrative actions, newest first. Entries never hold a password, a link or a file name.</p>
      <form className="audit-filters" onSubmit={apply}>
        <div>
          <label htmlFor={`${id}-a`}>Action</label>
          <select id={`${id}-a`} value={form.action} onChange={set("action")}>
            <option value="">Any action</option>
            {actions.map((a) => (
              <option key={a} value={a}>
                {actionLabel(a)}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={`${id}-w`}>Done by</label>
          <input id={`${id}-w`} value={form.actor} placeholder="email" onChange={set("actor")} />
        </div>
        <div>
          <label htmlFor={`${id}-t`}>About</label>
          <input id={`${id}-t`} value={form.target} placeholder="email or id" onChange={set("target")} />
        </div>
        <div>
          <label htmlFor={`${id}-f`}>From</label>
          <input id={`${id}-f`} type="date" value={form.from} onChange={set("from")} />
        </div>
        <div>
          <label htmlFor={`${id}-u`}>Until</label>
          <input id={`${id}-u`} type="date" value={form.to} onChange={set("to")} />
        </div>
        <div className="audit-filter-actions">
          <button type="submit" className="btn btn-primary" disabled={busy}>
            Filter
          </button>
          <button type="button" className="btn btn-quiet" onClick={clear}>
            Clear
          </button>
        </div>
      </form>
      <div role="alert" className="form-error">
        {error}
      </div>
      {items === null ? (
        !error && (
          <p className="muted" role="status">
            Loading…
          </p>
        )
      ) : items.length === 0 ? (
        <p className="muted">Nothing matches.</p>
      ) : (
        <ul className="audit-list">
          {items.map((e) => (
            <li key={e.id}>
              <div className="audit-head">
                <strong>{actionLabel(e.action)}</strong>
                <span className="muted small">{when(e.at)}</span>
              </div>
              <div className="small muted">by {e.actor_email ?? "the system"}</div>
              {detailLines(e).map((l, i) => (
                <div key={i} className="small">
                  {l}
                </div>
              ))}
            </li>
          ))}
        </ul>
      )}
      {next !== null && (
        <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => void more()}>
          Load more
        </button>
      )}
    </section>
  );
}
