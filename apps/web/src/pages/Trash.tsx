import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, isScope, type Target, type TrashItem } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Ellipsis, Trash2 } from "lucide-react";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { Menu, MenuItem } from "../ui/Menu";
import { Empty, Loading } from "../settings/kit";
import { Grid } from "../components/Grid";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { useSelection } from "../selection";
import { trashCaption } from "../trashText";

const PAGE = 60;

const photos = (n: number) => `${n} photo${n === 1 ? "" : "s"}`;

type Confirm = { kind: "forever"; target: Target; count: number } | { kind: "empty" };

/** Trashed photos: select to restore or delete forever, or empty the whole trash. */
export function Trash() {
  const { setAnonymous } = useAuth();
  const [items, setItems] = useState<TrashItem[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [total, setTotal] = useState(0); // everything in the trash, not just what is loaded
  const [retention, setRetention] = useState<number | null>(null);
  const [done, setDone] = useState(false);
  const [started, setStarted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const sentinel = useRef<HTMLDivElement>(null);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, total);
  const { toast, show } = useToast();
  const [now] = useState(() => Date.now()); // captions count days from when the page was opened

  const loadMore = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      const page = await api.listTrash({ cursor, limit: PAGE });
      setItems((cur) => {
        const seen = new Set(cur.map((i) => i.id));
        return [...cur, ...page.items.filter((i) => !seen.has(i.id))];
      });
      setCursor(page.next_cursor);
      setDone(page.next_cursor === null);
      setTotal(page.total);
      setRetention(page.retention_days);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not load the trash.");
    } finally {
      inFlight.current = false;
      setLoading(false);
      setStarted(true);
    }
  }, [cursor, setAnonymous]);

  useEffect(() => {
    void loadMore(); // first page, on mount only
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const el = sentinel.current;
    if (!el || !started || done || error) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) void loadMore();
      },
      { rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [started, done, error, items.length, loadMore]);

  const refreshThumb = useCallback(async () => {
    /* Expired thumbnail URLs: the trash has no per-item endpoint, so reload the first page's URLs lazily. */
    try {
      const page = await api.listTrash({ limit: PAGE });
      const fresh = new Map(page.items.map((i) => [i.id, i]));
      setItems((cur) => cur.map((i) => (fresh.has(i.id) ? { ...i, thumb_url: fresh.get(i.id)!.thumb_url } : i)));
    } catch {
      /* leave the tile as is */
    }
  }, []);

  // Esc clears the selection (the trash page is always in selection mode, so it never "leaves" it).
  useEffect(() => {
    if (confirm) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" && sel.count > 0) {
        e.preventDefault();
        sel.clear();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [confirm, sel]);

  const fail = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  // Drop what an action took out of the trash. After a whole-view action (the server did the work on photos that
  // were never loaded) the list starts over from the first page.
  const remove = useCallback(
    async (target: Target, count: number) => {
      if (isScope(target)) {
        const page = await api.listTrash({ limit: PAGE });
        setItems(page.items);
        setCursor(page.next_cursor);
        setDone(page.next_cursor === null);
        setTotal(page.total);
        return;
      }
      const gone = new Set(target);
      setItems((cur) => cur.filter((i) => !gone.has(i.id)));
      setTotal((t) => Math.max(0, t - count));
    },
    [],
  );

  const restore = useCallback(async () => {
    const count = sel.count;
    if (count === 0) return;
    const target = sel.target("trash");
    setBusy(true);
    try {
      await api.restoreAssets(target);
      await remove(target, count);
      show({ message: `${count} restored to your library` });
    } catch (e) {
      fail(e, "Couldn’t restore.");
    } finally {
      setBusy(false);
    }
  }, [sel, remove, show, fail]);

  const runConfirmed = useCallback(async () => {
    if (!confirm) return;
    setBusy(true);
    try {
      if (confirm.kind === "forever") {
        await api.purgeAssets(confirm.target);
        await remove(confirm.target, confirm.count);
        show({ message: `${confirm.count} deleted forever` });
      } else {
        const { count } = await api.emptyTrash();
        setItems([]);
        setTotal(0);
        setCursor(null);
        setDone(true);
        show({ message: `Trash emptied · ${count} deleted forever` });
      }
      setConfirm(null);
    } catch (e) {
      setConfirm(null);
      fail(e, "Couldn’t delete.");
    } finally {
      setBusy(false);
    }
  }, [confirm, remove, show, fail]);

  const header = (
    <PageHeader
      title="Recently deleted"
      actions={
        total > 0 && (
          <Menu
            label="More"
            trigger={
              <Button variant="ghost" size="icon" aria-label="More">
                <Ellipsis />
              </Button>
            }
          >
            <MenuItem danger onSelect={() => setConfirm({ kind: "empty" })}>
              <Trash2 />
              Empty trash
            </MenuItem>
          </Menu>
        )
      }
    />
  );

  if (!started)
    return (
      <>
        {header}
        <Loading>Loading the trash…</Loading>
      </>
    );

  if (error && items.length === 0) {
    return (
      <>
        {header}
        <Empty
          role="alert"
          title="Couldn’t load the trash"
          action={
            <Button variant="primary" onClick={() => void loadMore()}>
              Try again
            </Button>
          }
        >
          {error}
        </Empty>
      </>
    );
  }

  return (
    <>
      {header}
      <div className="px-4 pb-10 md:px-8">
        <p className="pb-3 text-sm text-muted">
          {retention !== null
            ? `Photos here are deleted forever ${retention} day${retention === 1 ? "" : "s"} after you move them.`
            : "Photos here are deleted forever after a while."}
        </p>

        {items.length === 0 ? (
          <Empty>
            <p>The trash is empty.</p>
          </Empty>
        ) : (
          <>
            <SelectionBar count={sel.count} total={total} onSelectAll={sel.selectAll} onClear={sel.clear}>
              <Button variant="primary" disabled={sel.count === 0 || busy} onClick={() => void restore()}>
                Restore
              </Button>
              <Button
                variant="danger"
                disabled={sel.count === 0 || busy}
                onClick={() => setConfirm({ kind: "forever", target: sel.target("trash"), count: sel.count })}
              >
                Delete forever
              </Button>
            </SelectionBar>
            <Grid
              items={items}
              onOpen={() => {}}
              onThumbError={() => void refreshThumb()}
              selecting
              selected={sel.selected}
              onSelect={sel.select}
              caption={(it) => trashCaption(it, now)}
            />
            <div ref={sentinel} className="h-px" aria-hidden="true" />
            <p className="py-4 text-center text-sm text-muted" role="status">
              {loading ? "Loading more…" : done ? photos(items.length) : ""}
            </p>
          </>
        )}
        {error && items.length > 0 && (
          <p className="py-4 text-center text-sm" role="alert">
            {error}{" "}
            <Button size="sm" variant="secondary" onClick={() => void loadMore()}>
              Retry
            </Button>
          </p>
        )}
      </div>

      {confirm && (
        <ConfirmDialog
          title={confirm.kind === "empty" ? "Empty the trash?" : `Delete ${photos(confirm.count)} forever?`}
          confirmLabel={confirm.kind === "empty" ? `Delete ${photos(total)} forever` : "Delete forever"}
          danger
          busy={busy}
          onCancel={() => setConfirm(null)}
          onConfirm={() => void runConfirmed()}
        >
          <p>
            {confirm.kind === "empty"
              ? `All ${photos(total)} in the trash will be permanently deleted, including the original files. `
              : `${confirm.count === 1 ? "This photo" : "These photos"} will be permanently deleted, including the original files. `}
            <strong>This can’t be undone.</strong>
          </p>
        </ConfirmDialog>
      )}
      {toast}
    </>
  );
}
