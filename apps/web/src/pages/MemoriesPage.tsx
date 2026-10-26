import { CircleCheck, Eye, EyeOff, MoreHorizontal } from "lucide-react";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { SlideshowItem } from "../collections/SlideshowItem";
import { itemsSource } from "../slideshow/sources";
import { api, ApiError, type AssetListItem, type Memories } from "../api";
import { useAuth } from "../auth";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { Menu, MenuItem } from "../ui/Menu";
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

  const back = { to: "/", label: "Collections" };
  if (error && !data)
    return (
      <>
        <PageHeader title="On this day" back={back} />
        <Message title="Couldn’t load On this day" body={error} alert>
          <Button variant="primary" onClick={() => void load()}>
            Try again
          </Button>
        </Message>
      </>
    );
  if (!data)
    return (
      <>
        <PageHeader title="On this day" back={back} />
        <div className="[display:grid] grid-cols-3 gap-0.5 px-0.5 pt-4 md:grid-cols-6 md:px-6" role="status" aria-label="Loading">
          {Array.from({ length: 12 }, (_, i) => (
            <div key={i} className="aspect-square animate-pulse bg-surface" />
          ))}
        </div>
      </>
    );

  let offset = 0;
  return (
    <>
      <PageHeader
        title="On this day"
        subtitle="Photos taken on this date in earlier years"
        back={back}
        actions={
          <>
            {flat.length > 0 && (
              <Button
                variant="ghost"
                size="icon"
                aria-label="Select"
                aria-pressed={selecting}
                className={selecting ? "bg-surface text-accent" : undefined}
                onClick={() => setSelecting(!selecting)}
              >
                <CircleCheck />
              </Button>
            )}
            <Menu
              label="On this day options"
              trigger={
                <Button variant="ghost" size="icon" aria-label="More">
                  <MoreHorizontal />
                </Button>
              }
            >
              {flat.length > 0 && <SlideshowItem source={() => itemsSource(flat, "On this day")} />}
              <MenuItem onSelect={() => void toggleStrip(!data.enabled)}>
                {data.enabled ? <EyeOff /> : <Eye />}
                {data.enabled ? "Hide from Collections" : "Show in Collections"}
              </MenuItem>
            </Menu>
          </>
        }
      />
      <div className="pb-10">
        {!data.enabled && (
          <p className="mx-4 mt-3 rounded-card bg-surface px-4 py-3 text-sm text-muted md:mx-8" role="status">
            On this day is hidden from Collections. Use the menu to show it again.
          </p>
        )}
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
            <Button variant="primary" className="flex-1 md:flex-none" disabled={picked.size === 0} onClick={() => void dismiss()}>
              Not this one
            </Button>
          </SelectionBar>
        )}
        {data.years.length === 0 ? (
          <Message title="Nothing for today" body="Photos taken on today’s date in earlier years will show up here." />
        ) : (
          data.years.map((y) => {
            const start = offset;
            offset += y.items.length;
            return (
              <section key={y.year} id={`y${y.year}`} aria-label={yearsAgo(y.years_ago)} className="scroll-mt-28 pt-5">
                <h2 className="mb-2 flex items-baseline gap-2 px-4 text-[1.0625rem] font-semibold tracking-tight md:px-8">
                  {yearsAgo(y.years_ago)}
                  <span className="text-sm font-normal text-muted">
                    {y.year} · {memoryCount(y.count)}
                  </span>
                </h2>
                <div className="px-0.5 md:px-6">
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
                </div>
              </section>
            );
          })
        )}
      </div>
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
    </>
  );
}

function Message({ title, body, alert, children }: { title: string; body: string; alert?: boolean; children?: ReactNode }) {
  return (
    <div className="mx-auto flex max-w-sm flex-col items-center gap-3 px-6 py-20 text-center" role={alert ? "alert" : undefined}>
      <h2 className="text-lg font-semibold tracking-tight">{title}</h2>
      <p className="text-[0.9375rem] text-muted">{body}</p>
      {children}
    </div>
  );
}
