import { useCallback, useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { SlideshowButton } from "../slideshow/SlideshowHost";
import { itemsSource } from "../slideshow/sources";
import { api, ApiError, type AssetListItem, type Memories } from "../api";
import { useAuth } from "../auth";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { LibraryTabs } from "../components/LibraryTabs";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { similarUrl } from "../searchText";
import { memoryCount, yearsAgo } from "../viewText";

/** Everything taken on today's date in earlier years, a section per year. "Not this one" takes photos out of memories. */
export function MemoriesPage() {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const { hash } = useLocation();
  const [data, setData] = useState<Memories | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const { toast, show } = useToast();

  const load = useCallback(async () => {
    try {
      setData(await api.memories({ all: true }));
      setError(null);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Couldn’t load On this day.");
    }
  }, [setAnonymous]);
  useEffect(() => {
    void load();
  }, [load]);

  // The strip links to one year: scroll there once the sections exist.
  useEffect(() => {
    if (data && hash) document.getElementById(hash.slice(1))?.scrollIntoView?.();
  }, [data, hash]);

  const flat = useMemo(() => (data?.years ?? []).flatMap((y) => y.items), [data]);
  const refreshThumb = useCallback(() => void load(), [load]);

  const dismiss = useCallback(async () => {
    const ids = [...picked];
    setSelecting(false);
    setPicked(new Set());
    try {
      const r = await api.dismissMemories(ids);
      await load();
      show({
        message: `${ids.length} taken out of On this day`,
        action: {
          label: "Undo",
          onClick: () => void api.dismissMemories(ids, false).then(load).catch(() => undefined),
        },
      });
      void r;
    } catch (e) {
      show({ tone: "error", message: `Couldn’t update. ${e instanceof Error ? e.message : ""}`.trim() });
    }
  }, [picked, load, show]);

  const toggleStrip = async (on: boolean) => {
    setData((d) => (d ? { ...d, enabled: on } : d));
    try {
      await api.setPrefs({ memories_enabled: on });
    } catch {
      setData((d) => (d ? { ...d, enabled: !on } : d));
    }
  };

  if (error && !data)
    return (
      <div className="empty" role="alert">
        <h1>Couldn’t load On this day</h1>
        <p className="muted">{error}</p>
        <button type="button" className="btn btn-primary" onClick={() => void load()}>
          Try again
        </button>
      </div>
    );
  if (!data) return <p className="center muted" role="status">Loading…</p>;

  let offset = 0;
  return (
    <div className="library memories-page">
      <LibraryTabs />
      <div className="page-head">
        <div>
          <h1 className="view-title">On this day</h1>
          <p className="muted small">Photos taken on this date in earlier years.</p>
        </div>
        {flat.length > 0 && (
          <div className="album-head-actions">
            <SlideshowButton source={() => itemsSource(flat, "On this day")} />
            <button type="button" className="btn btn-quiet" aria-pressed={selecting} onClick={() => setSelecting(!selecting)}>
              Select
            </button>
          </div>
        )}
      </div>
      <label className="sheet-check memories-switch">
        <input type="checkbox" checked={data.enabled} onChange={(e) => void toggleStrip(e.target.checked)} />
        <span>Show On this day at the top of the library</span>
      </label>
      {selecting && (
        <SelectionBar
          count={picked.size}
          total={flat.length}
          onSelectAll={() => setPicked(new Set(flat.map((i) => i.id)))}
          onClear={() => setPicked(new Set())}
          onDone={() => {
            setSelecting(false);
            setPicked(new Set());
          }}
        >
          <button type="button" className="btn btn-primary" disabled={picked.size === 0} onClick={() => void dismiss()}>
            Not this one
          </button>
        </SelectionBar>
      )}
      {data.years.length === 0 ? (
        <div className="empty">
          <h2>Nothing for today</h2>
          <p>Photos taken on today’s date in earlier years will show up here.</p>
        </div>
      ) : (
        data.years.map((y) => {
          const start = offset;
          offset += y.items.length;
          return (
            <section key={y.year} id={`y${y.year}`} className="memory-year" aria-label={yearsAgo(y.years_ago)}>
              <h2>
                {yearsAgo(y.years_ago)} <span className="muted small">{y.year} · {memoryCount(y.count)}</span>
              </h2>
              <Grid<AssetListItem>
                items={y.items}
                onOpen={(i) => setOpen(start + i)}
                onThumbError={refreshThumb}
                selecting={selecting}
                selected={picked}
                onSelect={(id) =>
                  setPicked((p) => {
                    const n = new Set(p);
                    if (!n.delete(id)) n.add(id);
                    return n;
                  })
                }
              />
            </section>
          );
        })
      )}
      {open !== null && flat[open] && (
        <Lightbox
          items={flat}
          index={open}
          onIndexChange={setOpen}
          onClose={() => setOpen(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
      {toast}
    </div>
  );
}
