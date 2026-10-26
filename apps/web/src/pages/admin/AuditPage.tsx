import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { api, type AuditEntry, type AuditQuery } from "../../api";
import { actionLabel, detailLines } from "../../auditText";
import { Field, FormError, Group, inputClass, Loading, Row, SelectBox } from "../../settings/kit";
import { Button } from "../../ui/Button";

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
    <section aria-label="Audit log" className="[overflow-wrap:anywhere]">
      <p className="text-sm text-muted">Administrative actions, newest first. Entries never hold a password, a link or a file name.</p>
      <form className="mt-4 grid grid-cols-1 gap-3 rounded-card bg-card p-4 ring-1 ring-line min-[560px]:grid-cols-2" onSubmit={apply}>
        <Field label="Action">
          {(p) => (
            <SelectBox {...p} value={form.action} onChange={set("action")}>
              <option value="">Any action</option>
              {actions.map((a) => (
                <option key={a} value={a}>
                  {actionLabel(a)}
                </option>
              ))}
            </SelectBox>
          )}
        </Field>
        <Field label="Done by">
          {(p) => <input {...p} className={inputClass} value={form.actor} placeholder="email" onChange={set("actor")} />}
        </Field>
        <Field label="About">
          {(p) => <input {...p} className={inputClass} value={form.target} placeholder="email or id" onChange={set("target")} />}
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="From">{(p) => <input {...p} className={inputClass} type="date" value={form.from} onChange={set("from")} />}</Field>
          <Field label="Until">{(p) => <input {...p} className={inputClass} type="date" value={form.to} onChange={set("to")} />}</Field>
        </div>
        <div className="flex flex-wrap gap-2 min-[560px]:col-span-2">
          <Button type="submit" variant="primary" disabled={busy}>
            Filter
          </Button>
          <Button variant="quiet" onClick={clear}>
            Clear
          </Button>
        </div>
      </form>
      <div className="mt-3">
        <FormError>{error}</FormError>
      </div>
      {items === null ? (
        !error && <Loading />
      ) : items.length === 0 ? (
        <p className="mt-4 text-sm text-muted">Nothing matches.</p>
      ) : (
        <Group as="ul" className="mt-4">
          {items.map((e) => (
            <Row
              as="li"
              key={e.id}
              className="py-3"
              label={
                <span className="flex flex-wrap items-baseline justify-between gap-x-3">
                  <strong className="font-medium">{actionLabel(e.action)}</strong>
                  <span className="text-[0.8125rem] font-normal text-muted">{when(e.at)}</span>
                </span>
              }
              detail={
                <>
                  <span className="block">by {e.actor_email ?? "the system"}</span>
                  {detailLines(e).map((l, i) => (
                    <span key={i} className="block text-fg">
                      {l}
                    </span>
                  ))}
                </>
              }
            />
          ))}
        </Group>
      )}
      {next !== null && (
        <div className="mt-4">
          <Button disabled={busy} onClick={() => void more()}>
            Load more
          </Button>
        </div>
      )}
    </section>
  );
}
