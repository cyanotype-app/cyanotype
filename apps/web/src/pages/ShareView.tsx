import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { useParams } from "react-router-dom";
import { ApiError } from "../api";
import { photoCount } from "../albumText";
import { AuthShell } from "../components/AuthShell";
import { Logo } from "../components/Logo";
import { ChevronLeft, ChevronRight, Download, X } from "lucide-react";
import { Empty, Field, FormError, inputClass, Loading } from "../settings/kit";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { ZoomControls } from "../components/ZoomControls";
import { fitStyle } from "../photoFit";
import { usePhotoZoom } from "../usePhotoZoom";
import { fileSize } from "../pairText";
import { publicDownloadHref, publicZipHref, shareApi, type PublicInfo, type PublicItem } from "../shareApi";
import { expiryText } from "../shareText";
import { placeholderUrl } from "../thumbhash";
import { followLink, planShareZip, sharePlanMessage } from "../zipDownload";
import { SlideshowButton, SlideshowProvider } from "../slideshow/SlideshowHost";
import { pagedSource } from "../slideshow/sources";

type Phase =
  | { kind: "loading" }
  /** Unknown, revoked and expired links all look the same, on purpose. */
  | { kind: "gone" }
  | { kind: "error"; message: string }
  | { kind: "locked" }
  | { kind: "open"; info: PublicInfo };

/** Keep search engines and referrers away from a page whose address is the secret. */
function usePrivatePage(title: string) {
  useEffect(() => {
    const metas = [
      ["robots", "noindex, nofollow, noarchive"],
      ["referrer", "no-referrer"],
    ].map(([name, content]) => {
      const m = document.createElement("meta");
      m.name = name!;
      m.content = content!;
      document.head.appendChild(m);
      return m;
    });
    return () => metas.forEach((m) => m.remove());
  }, []);
  useEffect(() => {
    const prev = document.title;
    document.title = title;
    return () => {
      document.title = prev;
    };
  }, [title]);
}

/** The public page of a share link (`/s/<token>`): no login, a grid, a viewer, downloads when the owner allows them. */
export function ShareView() {
  const { token = "" } = useParams();
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const title = phase.kind === "open" ? (phase.info.title ?? "Shared photos") : "Shared photos";
  usePrivatePage(`${title} · Cyanotype`);

  const load = useCallback(async () => {
    setPhase({ kind: "loading" });
    try {
      const info = await shareApi.publicInfo(token);
      setPhase(info.unlocked ? { kind: "open", info } : { kind: "locked" });
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) setPhase({ kind: "gone" });
      else setPhase({ kind: "error", message: e instanceof Error ? e.message : "Could not load this link." });
    }
  }, [token]);
  useEffect(() => {
    void load();
  }, [load]);

  return (
    <SlideshowProvider>
      {phase.kind === "loading" && (
        <main className="flex min-h-dvh items-center justify-center bg-bg">
          <Loading />
        </main>
      )}
      {phase.kind === "gone" && (
        <AuthShell title="This link is not available">
          <p className="text-center text-[0.9375rem] text-muted">It may have expired or been turned off by its owner.</p>
        </AuthShell>
      )}
      {phase.kind === "error" && (
        <AuthShell title="Something went wrong">
          <div className="flex flex-col items-center gap-4">
            <p role="alert" className="text-center text-[0.9375rem] text-muted">
              {phase.message}
            </p>
            <Button variant="primary" onClick={() => void load()}>
              Try again
            </Button>
          </div>
        </AuthShell>
      )}
      {phase.kind === "locked" && <PasswordForm token={token} onUnlocked={() => void load()} />}
      {phase.kind === "open" && <Photos token={token} info={phase.info} onLocked={load} />}
    </SlideshowProvider>
  );
}

function PasswordForm({ token, onUnlocked }: { token: string; onUnlocked: () => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!password) return;
    setBusy(true);
    setError(null);
    try {
      await shareApi.publicUnlock(token, password);
      onUnlocked();
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) setError("This link is not available.");
      else if (err instanceof ApiError && err.status === 401) setError("That password is not right.");
      else if (err instanceof ApiError && err.status === 429) setError("Too many attempts. Wait a few minutes and try again.");
      else setError(err instanceof Error ? err.message : "Could not check the password.");
      setBusy(false);
    }
  }

  return (
    <AuthShell title="This link is protected">
      <form className="flex flex-col gap-4" onSubmit={(e) => void submit(e)}>
        <Field label="Password">
          {(p) => (
            <input
              {...p}
              className={inputClass}
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={busy}
              autoFocus
            />
          )}
        </Field>
        <FormError>{error}</FormError>
        <Button type="submit" variant="primary" size="lg" disabled={busy || !password}>
          View photos
        </Button>
      </form>
    </AuthShell>
  );
}

function Photos({ token, info, onLocked }: { token: string; info: PublicInfo; onLocked: () => void }) {
  const [items, setItems] = useState<PublicItem[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const loading = useRef(false);
  const lastRefresh = useRef(0);
  const sentinel = useRef<HTMLDivElement>(null);

  const handle = useCallback(
    (e: unknown) => {
      if (e instanceof ApiError && e.status === 401) onLocked();
      else setError(e instanceof Error ? e.message : "Could not load the photos.");
    },
    [onLocked],
  );

  const more = useCallback(
    async (from: string | null) => {
      if (loading.current) return;
      loading.current = true;
      setError(null);
      try {
        const page = await shareApi.publicItems(token, from);
        setItems((prev) => (from ? [...prev, ...page.items.filter((i) => !prev.some((p) => p.id === i.id))] : page.items));
        setCursor(page.next_cursor);
        setLoaded(true);
      } catch (e) {
        handle(e);
      } finally {
        loading.current = false;
      }
    },
    [token, handle],
  );
  useEffect(() => {
    void more(null);
  }, [more]);

  // Presigned image URLs last ten minutes: when one fails, fetch the loaded pages again (at most every 20 s).
  const refresh = useCallback(async () => {
    if (Date.now() - lastRefresh.current < 20_000) return;
    lastRefresh.current = Date.now();
    try {
      let next: string | null = null;
      const fresh: PublicItem[] = [];
      do {
        const page = await shareApi.publicItems(token, next);
        fresh.push(...page.items);
        next = page.next_cursor;
      } while (next && fresh.length < items.length);
      setItems(fresh);
      setCursor(next);
    } catch (e) {
      handle(e);
    }
  }, [token, items.length, handle]);

  useEffect(() => {
    const el = sentinel.current;
    if (!el || !cursor || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) void more(cursor);
    }, { rootMargin: "600px" });
    io.observe(el);
    return () => io.disconnect();
  }, [cursor, more]);

  const update = useCallback((item: PublicItem) => setItems((prev) => prev.map((p) => (p.id === item.id ? item : p))), []);

  return (
    <div className="min-h-dvh bg-bg text-fg">
      <header className="glass sticky top-0 z-[5] flex h-12 items-center gap-2 px-4 pt-[env(safe-area-inset-top,0px)] md:px-8">
        <Logo size={22} />
        <span className="text-[0.9375rem] font-semibold tracking-tight">Cyanotype</span>
      </header>
      <main className="mx-auto max-w-6xl px-1 pb-12 md:px-8">
        <div className="flex flex-col gap-4 px-3 pt-5 pb-5 md:px-0 md:pt-8">
          <div>
            <h1 className="text-[1.75rem] leading-tight font-bold tracking-[-0.02em] break-words md:text-[2rem]">{info.title ?? "Shared photos"}</h1>
            <p className="mt-1 text-sm text-muted">
              {[photoCount(info.item_count), info.expires_at ? expiryText(info.expires_at) : null].filter(Boolean).join(" · ")}
            </p>
          </div>
          <div className="flex flex-wrap items-start gap-2">
            {info.allow_download && info.item_count > 0 && <DownloadAll token={token} onLocked={onLocked} />}
            {items.length > 0 && <SlideshowButton source={() => shareSource(token, info.title ?? "Shared photos")} />}
          </div>
        </div>
        {loaded && items.length === 0 && !error && (
          <Empty title="Nothing to see here">
            <p>There are no photos on this link right now.</p>
          </Empty>
        )}
        {items.length > 0 && (
          <ul className="grid grid-cols-3 gap-0.5 sm:grid-cols-4 md:grid-cols-5 md:gap-1 lg:grid-cols-6" aria-label="Photos">
            {items.map((it, i) => {
              const bg = placeholderUrl(it.thumbhash);
              return (
                <li key={it.id}>
                  <button
                    type="button"
                    className="relative block aspect-square w-full overflow-hidden bg-surface bg-cover md:rounded-tile"
                    aria-label={`Open photo ${i + 1} of ${info.item_count}`}
                    style={bg ? { backgroundImage: `url(${bg})` } : undefined}
                    onClick={() => setOpen(i)}
                  >
                    {it.thumb_url ? (
                      <img
                        src={it.thumb_url}
                        alt=""
                        loading="lazy"
                        className="size-full object-cover transition-transform duration-300 ease-soft hover:scale-[1.03]"
                        onError={() => void refresh()}
                      />
                    ) : (
                      <span className="absolute inset-0 grid grid-cols-1 place-items-center text-xs text-muted">Processing…</span>
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
        <div ref={sentinel} className="h-px" aria-hidden="true" />
        {cursor && typeof IntersectionObserver === "undefined" && (
          <div className="flex justify-center py-6">
            <Button onClick={() => void more(cursor)}>Show more</Button>
          </div>
        )}
        {error && (
          <p className="py-6 text-center text-sm" role="alert">
            {error}{" "}
            <Button size="sm" onClick={() => void more(cursor)}>
              Retry
            </Button>
          </p>
        )}
      </main>
      {open !== null && items[open] && (
        <ShareLightbox
          token={token}
          items={items}
          index={Math.min(open, items.length - 1)}
          total={info.item_count}
          allowDownload={info.allow_download}
          onIndexChange={setOpen}
          onClose={() => setOpen(null)}
          onItem={update}
          onImageError={() => void refresh()}
        />
      )}
    </div>
  );
}

/**
 * "Download all": every photo of the link the owner lets visitors download, as one ZIP. The page first asks the server what it
 * holds (and waits for the download copies of JPEGs it is still making), then follows a plain link, so the browser streams the
 * archive to disk and shows its own progress.
 */
function DownloadAll({ token, onLocked }: { token: string; onLocked: () => void }) {
  const [state, setState] = useState<{ kind: "idle" } | { kind: "busy"; note: string } | { kind: "done"; note: string } | { kind: "error"; note: string }>({ kind: "idle" });

  async function start() {
    setState({ kind: "busy", note: "Preparing the download…" });
    try {
      const plan = await planShareZip(() => shareApi.publicZipPlan(token), {
        onWaiting: (n) => setState({ kind: "busy", note: `Preparing the download… (${n.toLocaleString("en-US")} ${n === 1 ? "photo" : "photos"} to go)` }),
      });
      if (plan.files === 0) {
        setState({ kind: "error", note: "None of the photos on this link can be downloaded." });
        return;
      }
      followLink(publicZipHref(token));
      setState({ kind: "done", note: sharePlanMessage(plan) });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) onLocked();
      else if (e instanceof ApiError && e.status === 429) setState({ kind: "error", note: "Too many downloads just now. Wait a minute and try again." });
      else setState({ kind: "error", note: e instanceof Error ? e.message : "Could not start the download." });
    }
  }

  return (
    <div className="flex flex-col items-start gap-2">
      <Button variant="primary" disabled={state.kind === "busy"} onClick={() => void start()}>
        <Download />
        {state.kind === "busy" ? "Preparing…" : "Download all"}
      </Button>
      {state.kind === "error" ? (
        <p role="alert" className="max-w-md text-sm text-danger">
          {state.note}
        </p>
      ) : (
        state.kind !== "idle" && (
          <p role="status" className="max-w-md text-sm text-muted">
            {state.note}
          </p>
        )
      )}
    </div>
  );
}

/**
 * The photos of a share link for a slideshow. Each one's preview address is asked for just before it is shown (they last ten
 * minutes), and a link whose password session ended says so instead of offering a sign-in.
 */
function shareSource(token: string, title: string) {
  return {
    ...pagedSource(
      async (cursor) => {
        const page = await shareApi.publicItems(token, cursor);
        return { items: page.items, next_cursor: page.next_cursor };
      },
      title,
      {
        resolve: async (item) => {
          const fresh = await shareApi.publicItem(token, item.id);
          return fresh.preview_url ? { url: fresh.preview_url } : null;
        },
      },
    ),
    expired: { title: "This link is locked", text: "Open the link again and enter its password." },
  };
}

/** How often and how long to ask whether the download copy of a JPEG is ready. */
const POLL_MS = 1500;
const POLL_TRIES = 40;

function ShareLightbox({
  token,
  items,
  index,
  total,
  allowDownload,
  onIndexChange,
  onClose,
  onItem,
  onImageError,
}: {
  token: string;
  items: PublicItem[];
  index: number;
  total: number;
  allowDownload: boolean;
  onIndexChange: (i: number) => void;
  onClose: () => void;
  onItem: (item: PublicItem) => void;
  onImageError: () => void;
}) {
  const item = items[index]!;
  const dialogRef = useRef<HTMLDivElement>(null);
  const [loadedId, setLoadedId] = useState<string | null>(null);
  // The size of the picture on show (read when it loads): the CSS fits the photo to the area from its shape.
  const [photoSize, setPhotoSize] = useState<{ id: string; w: number; h: number } | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [gaveUp, setGaveUp] = useState<string | null>(null);
  // Zoom the photo (pinch, ctrl + wheel, double tap, + - z, the buttons). A share link has no sharper picture than the
  // preview (its downloads are attachments, not for display), so zooming scales the preview.
  const stageRef = useRef<HTMLDivElement>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const levelRef = useRef<HTMLSpanElement>(null);
  const announceRef = useRef<HTMLSpanElement>(null);
  const previewLong = photoSize?.id === item.id ? Math.max(photoSize.w, photoSize.h) : 0;
  const zoom = usePhotoZoom({
    stage: stageRef,
    layer: layerRef,
    photo: imgRef,
    level: levelRef,
    announce: announceRef,
    resetKey: item.id,
    active: true,
    previewLong,
    naturalLong: previewLong,
  });
  const go = useCallback(
    (d: number) => {
      const n = index + d;
      if (n >= 0 && n < items.length) onIndexChange(n);
    },
    [index, items.length, onIndexChange],
  );

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (["+", "=", "-", "_", "z"].includes(e.key) && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        if (e.key === "+" || e.key === "=") zoom.zoomIn();
        else if (e.key === "-" || e.key === "_") zoom.zoomOut();
        else if (!e.repeat) zoom.toggle();
      } else if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      } else if (e.key === "Tab" && dialogRef.current) {
        const f = dialogRef.current.querySelectorAll<HTMLElement>("button:not(:disabled), a[href]");
        const first = f[0];
        const last = f[f.length - 1];
        if (!first || !last) return;
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [go, onClose, zoom]);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    dialogRef.current?.querySelector<HTMLElement>("button")?.focus();
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = overflow;
      prev?.focus?.();
    };
  }, []);

  // A JPEG's download is a copy without location that the server makes on first need: ask for it, then wait for it.
  const pending = allowDownload && item.download_pending;
  const itemId = item.id;
  useEffect(() => {
    if (!pending) return;
    let live = true;
    let tries = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      tries++;
      try {
        const fresh = await shareApi.publicItem(token, itemId);
        if (!live) return;
        if (!fresh.download_pending) return onItem(fresh);
      } catch {
        /* try again */
      }
      if (!live) return;
      if (tries >= POLL_TRIES) setGaveUp(itemId);
      else timer = setTimeout(() => void poll(), POLL_MS);
    };
    shareApi.publicPrepare(token, itemId).then(
      () => {
        if (live) timer = setTimeout(() => void poll(), 800);
      },
      () => live && setGaveUp(itemId),
    );
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [pending, token, itemId, onItem]);

  const downloads = allowDownload ? item.downloads : [];
  const src = item.preview_url ?? item.thumb_url;
  const fit = photoSize?.id === item.id ? photoSize : null;
  const bg = placeholderUrl(item.thumbhash);
  const date = new Date(item.sort_time);
  const menuOpen = menuFor === item.id;

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-[#05090d] text-white"
      role="dialog"
      aria-modal="true"
      aria-label="Photo"
      ref={dialogRef}
    >
      <div className="relative z-[3] flex items-center justify-between gap-2 px-3 pt-[max(0.75rem,env(safe-area-inset-top,0px))] pb-2">
        <Button variant="overlay" onClick={onClose}>
          <X />
          Close
        </Button>
        <span className="pointer-events-none absolute left-1/2 -translate-x-1/2 text-sm text-white/70 tabular-nums" aria-live="polite">
          {index + 1} / {Math.max(total, items.length)}
        </span>
        <div className="flex items-center gap-2">
          {downloads.length === 1 && (
            <Button variant="overlay" asChild>
              <a href={publicDownloadHref(token, item.id, downloads[0]!.id)} rel="noopener">
                <Download />
                Download
              </a>
            </Button>
          )}
          {downloads.length > 1 && (
            <Button variant="overlay" aria-expanded={menuOpen} aria-controls="share-downloads" onClick={() => setMenuFor(menuOpen ? null : item.id)}>
              <Download />
              Download
            </Button>
          )}
          {pending && (
            <Button variant="overlay" disabled>
              {gaveUp === item.id ? "Download unavailable" : "Preparing download…"}
            </Button>
          )}
        </div>
      </div>
      {downloads.length > 1 && menuOpen && (
        <div
          className="absolute top-[max(3.75rem,calc(env(safe-area-inset-top,0px)+3rem))] right-3 z-[4] flex flex-col gap-1 rounded-card bg-black/70 p-1.5 backdrop-blur-md"
          id="share-downloads"
        >
          {downloads.map((d) => (
            <Button key={d.id} variant="overlay" className="justify-start" asChild>
              <a href={publicDownloadHref(token, item.id, d.id)} rel="noopener">
                Download {d.label} {d.size_bytes !== null && <span className="text-white/60">{fileSize(d.size_bytes)}</span>}
              </a>
            </Button>
          ))}
        </div>
      )}
      <div
        className={cn("relative min-h-0 flex-1 touch-none overflow-hidden bg-cover bg-center [container-type:size]", zoom.zoomed && "cursor-grab active:cursor-grabbing")}
        ref={stageRef}
        {...zoom.handlers}
        style={bg ? { backgroundImage: `url(${bg})` } : undefined}
      >
        <div className="absolute inset-0 bg-[rgb(0_10_17/0.8)]" aria-hidden="true" />
        {src ? (
          <div
            className="[--fit-w:calc(100cqw-env(safe-area-inset-left,0px)-env(safe-area-inset-right,0px))] absolute inset-y-0 right-[env(safe-area-inset-right,0px)] left-[env(safe-area-inset-left,0px)] flex origin-center items-center justify-center [&.easing]:transition-transform [&.easing]:duration-[180ms] [&.easing]:ease-out [&.gesturing]:will-change-transform"
            ref={layerRef}
          >
            <img
              key={item.id}
              ref={imgRef}
              src={src}
              alt=""
              className={cn(
                "max-h-full max-w-full flex-none object-contain select-none [-webkit-user-drag:none] transition-opacity duration-200",
                item.preview_url && loadedId !== item.id && "opacity-0",
                fit &&
                  "h-auto max-h-none w-[min(var(--fit-w),calc(100cqh*var(--ar)),var(--cap))] max-w-none [aspect-ratio:var(--ar)]",
              )}
              style={fitStyle(fit)}
              draggable={false}
              onLoad={(e) => {
                setLoadedId(item.id);
                setPhotoSize({ id: item.id, w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight });
              }}
              onError={onImageError}
            />
          </div>
        ) : (
          <p className="absolute inset-0 m-0 grid grid-cols-1 place-items-center text-sm text-white/70">This photo is still being processed.</p>
        )}
        <Button
          variant="overlay"
          size="icon"
          className="absolute top-1/2 left-2 z-[2] -translate-y-1/2 md:left-4"
          aria-label="Previous photo"
          onClick={() => go(-1)}
          disabled={index === 0}
        >
          <ChevronLeft />
        </Button>
        <Button
          variant="overlay"
          size="icon"
          className="absolute top-1/2 right-2 z-[2] -translate-y-1/2 md:right-4"
          aria-label="Next photo"
          onClick={() => go(1)}
          disabled={index >= items.length - 1}
        >
          <ChevronRight />
        </Button>
        {src && <ZoomControls zoomed={zoom.zoomed} onIn={zoom.zoomIn} onOut={zoom.zoomOut} onFit={zoom.fit} levelRef={levelRef} announceRef={announceRef} />}
      </div>
      <p className="m-0 px-4 pt-2 pb-[max(0.75rem,env(safe-area-inset-bottom,0px))] text-center text-sm text-white/70">
        {item.width && item.height ? `${item.width} × ${item.height} · ` : ""}
        <time dateTime={item.sort_time}>{Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString()}</time>
      </p>
    </div>
  );
}
