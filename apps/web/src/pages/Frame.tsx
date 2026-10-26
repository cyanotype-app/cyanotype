import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { api, ApiError, type Album } from "../api";
import { useAuth } from "../auth";
import {
  DEFAULT_INTERVAL,
  INTERVALS,
  frameParams,
  frameScope,
  loadFrameConfig,
  parseFrameParams,
  saveFrameConfig,
  type FrameConfig,
  type FrameSource,
} from "../frameConfig";
import { Logo } from "../components/Logo";
import { SlideshowController } from "../slideshow/controller";
import { scopeSource } from "../slideshow/sources";
import { enterFullscreen, exitFullscreen, fullscreenSupported, isFullscreen, useWakeLock } from "../slideshow/device";
import { makeLoader } from "../slideshow/loader";
import { PreviewCache } from "../slideshow/previews";
import { SlideStage } from "../slideshow/SlideStage";
import { useShowState } from "../slideshow/useShow";

/** The menu a tap opens closes itself after this long. */
const MENU_MS = 10_000;
/** While the session is unknown (just booted, no network yet), ask again this often. */
const PROBE_MS = 10_000;

const intervalLabel = (s: number) => (s < 60 ? `${s} seconds` : s < 3600 ? `${s / 60} minutes` : "1 hour");

/**
 * `/frame`: random photos on a wall tablet. Signed in with the normal account; the address (or what this browser remembered)
 * says what to show. Photos change by themselves, fitted over a blurred, darkened copy of themselves. Tap for a small menu.
 */
export function Frame() {
  const { state, refresh } = useAuth();
  const [params] = useSearchParams();
  const location = useLocation();
  const probe = useSessionProbe(state.status === "anonymous", refresh);

  if (state.status === "loading") return <FrameMessage title="Photo frame" text="Starting…" />;
  if (state.status === "anonymous") {
    if (probe === "signed-out") return <FrameSignIn from={location.pathname + location.search} />;
    return probe === "offline" ? (
      <FrameMessage title="Waiting for the network" text="The frame starts as soon as the server can be reached." />
    ) : (
      <FrameMessage title="Photo frame" text="Starting…" />
    );
  }
  const fromAddress = parseFrameParams(params);
  const config = params.has("setup") ? null : (fromAddress ?? (params.size === 0 ? loadFrameConfig() : null));
  if (!config) return <FrameSetup initial={fromAddress ?? loadFrameConfig()} />;
  return <FrameShow key={frameParams(config).toString()} config={config} />;
}

/** After a reboot the browser may start before the network: only a real "not signed in" answer shows the sign-in screen. */
function useSessionProbe(anonymous: boolean, refresh: () => Promise<void>): "checking" | "offline" | "signed-out" {
  const [result, setResult] = useState<"checking" | "offline" | "signed-out">("checking");
  useEffect(() => {
    if (!anonymous) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ask = async () => {
      try {
        await api.me();
        if (!cancelled) await refresh();
      } catch (e) {
        if (cancelled) return;
        if (e instanceof ApiError && e.status === 401) {
          setResult("signed-out");
          return;
        }
        setResult("offline");
        timer = setTimeout(() => void ask(), PROBE_MS);
      }
    };
    void ask();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [anonymous, refresh]);
  return result;
}

function FrameMessage({ title, text, children }: { title: string; text: string; children?: React.ReactNode }) {
  return (
    <main className="frame-message">
      <Logo size={40} />
      <h1>{title}</h1>
      <p className="muted" role="status">
        {text}
      </p>
      {children}
    </main>
  );
}

/** Shown when the session has ended: a plain screen with one button, never a blank page. */
function FrameSignIn({ from }: { from: string }) {
  return (
    <FrameMessage title="Sign in again" text="This device is no longer signed in, so the frame cannot show your photos.">
      <Link className="btn btn-primary" to="/login" state={{ from }}>
        Sign in
      </Link>
    </FrameMessage>
  );
}

// --- Setup ---------------------------------------------------------------------------------------------------------------

function FrameSetup({ initial }: { initial: FrameConfig | null }) {
  const navigate = useNavigate();
  const [source, setSource] = useState<FrameSource["kind"]>(initial?.source.kind ?? "library");
  const [albumId, setAlbumId] = useState(initial?.source.kind === "album" ? initial.source.id : "");
  const [interval, setIntervalSeconds] = useState<number>(initial?.interval ?? DEFAULT_INTERVAL);
  const [clock, setClock] = useState(initial?.clock ?? false);
  const [caption, setCaption] = useState(initial?.caption ?? false);
  const [albums, setAlbums] = useState<Album[] | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let live = true;
    api
      .listAlbums()
      .then((r) => live && setAlbums(r.items))
      .catch(() => live && setFailed(true));
    return () => {
      live = false;
    };
  }, []);

  const chosen: FrameSource | null =
    source === "album" ? (albumId ? { kind: "album", id: albumId } : null) : { kind: source };
  const config: FrameConfig | null = chosen ? { source: chosen, interval, clock, caption } : null;

  function start(e: FormEvent) {
    e.preventDefault();
    if (!config) return;
    saveFrameConfig(config);
    // Full screen needs this click; a tablet in kiosk mode already is.
    void enterFullscreen();
    navigate(`/frame?${frameParams(config)}`, { replace: true });
  }

  return (
    <main className="frame-setup">
      <header>
        <Logo size={32} />
        <h1>Photo frame</h1>
      </header>
      <p className="muted">Show random photos, one after another, on a screen that stays on. Photos in the trash, hidden photos and (from a whole album) archived ones are never shown.</p>
      <form onSubmit={start}>
        <fieldset>
          <legend>What to show</legend>
          <label className="frame-choice">
            <input type="radio" name="source" checked={source === "library"} onChange={() => setSource("library")} />
            <span>My whole library</span>
          </label>
          <label className="frame-choice">
            <input type="radio" name="source" checked={source === "favorites"} onChange={() => setSource("favorites")} />
            <span>My favorites</span>
          </label>
          <label className="frame-choice">
            <input type="radio" name="source" checked={source === "album"} onChange={() => setSource("album")} />
            <span>One album</span>
          </label>
          {source === "album" && (
            <div className="frame-album">
              {failed ? (
                <p className="muted" role="alert">
                  Could not load your albums.
                </p>
              ) : albums === null ? (
                <p className="muted" role="status">
                  Loading albums…
                </p>
              ) : albums.length === 0 ? (
                <p className="muted">You have no albums yet.</p>
              ) : (
                <select aria-label="Album" value={albumId} onChange={(e) => setAlbumId(e.target.value)}>
                  <option value="">Choose an album</option>
                  {albums.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.title}
                    </option>
                  ))}
                </select>
              )}
            </div>
          )}
        </fieldset>
        <label className="frame-field">
          <span>Change the photo every</span>
          <select value={interval} onChange={(e) => setIntervalSeconds(Number(e.target.value))}>
            {[...new Set<number>([...INTERVALS, interval])].sort((a, b) => a - b).map((s) => (
              <option key={s} value={s}>
                {intervalLabel(s)}
              </option>
            ))}
          </select>
        </label>
        <label className="frame-check">
          <input type="checkbox" checked={clock} onChange={(e) => setClock(e.target.checked)} />
          <span>Show the time and date</span>
        </label>
        <label className="frame-check">
          <input type="checkbox" checked={caption} onChange={(e) => setCaption(e.target.checked)} />
          <span>Show photo captions</span>
        </label>
        <div className="frame-actions">
          <button type="submit" className="btn btn-primary" disabled={!config}>
            Start the frame
          </button>
          <Link className="btn btn-quiet" to="/photos">
            Back to my library
          </Link>
        </div>
      </form>
      {config && (
        <p className="muted small frame-address">
          To start here every time, open <code>{`/frame?${frameParams(config)}`}</code> on the tablet and bookmark it.
        </p>
      )}
    </main>
  );
}

// --- The frame ---------------------------------------------------------------------------------------------------------------

function FrameShow({ config }: { config: FrameConfig }) {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const [ctl, setCtl] = useState<SlideshowController | null>(null);
  const state = useShowState(ctl);
  const [menu, setMenu] = useState(false);
  const menuTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const first = useRef<HTMLButtonElement>(null);
  // The source is made once per choice, not once per render of the page above (a new one would restart the show).
  const choice = config.source.kind === "album" ? `album:${config.source.id}` : config.source.kind;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const source = useMemo(() => scopeSource(frameScope(config.source), "Photo frame"), [choice]);
  const cache = useMemo(() => new PreviewCache(), []);
  useWakeLock(true);

  useEffect(() => {
    saveFrameConfig(config);
  }, [config]);

  useEffect(() => {
    const c = new SlideshowController({
      source,
      order: "shuffle",
      loop: true,
      durationMs: config.interval * 1000,
      transition: "crossfade",
      load: makeLoader(source, cache),
      reducedMotion: () => typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches,
      offline: () => navigator.onLine === false,
      // Archived photos are put away on purpose: an album lists them, the frame does not.
      skip: (item) => item.archived === true,
    });
    setCtl(c);
    c.start();
    const retry = () => c.retryNow();
    const visible = () => document.visibilityState === "visible" && c.retryNow();
    window.addEventListener("online", retry);
    document.addEventListener("visibilitychange", visible);
    return () => {
      window.removeEventListener("online", retry);
      document.removeEventListener("visibilitychange", visible);
      c.destroy();
      cache.clear();
      setCtl(null);
    };
  }, [source, cache, config.interval]);

  const openMenu = useCallback(() => {
    setMenu(true);
    if (menuTimer.current) clearTimeout(menuTimer.current);
    menuTimer.current = setTimeout(() => setMenu(false), MENU_MS);
  }, []);
  const closeMenu = useCallback(() => {
    setMenu(false);
    if (menuTimer.current) clearTimeout(menuTimer.current);
  }, []);
  useEffect(() => () => void (menuTimer.current && clearTimeout(menuTimer.current)), []);
  useEffect(() => {
    if (menu) first.current?.focus();
  }, [menu]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") (menu ? closeMenu : openMenu)();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu, openMenu, closeMenu]);

  // The session ended: tell the app too, so the sign-in page (which sends a signed-in visitor straight back) lets the person in.
  const signedOut = state.status === "signed-out";
  useEffect(() => {
    if (signedOut) setAnonymous();
  }, [signedOut, setAnonymous]);
  if (signedOut) return <FrameSignIn from={location.pathname + location.search} />;

  const { slide } = state;
  return (
    <div className="frame" onClick={() => (menu ? undefined : openMenu())}>
      <SlideStage slide={slide} outgoing={state.outgoing} fade={state.fade} backdrop="blur" />
      {(config.clock || (config.caption && slide?.caption)) && (
        <div className="frame-info">
          {config.clock && <FrameClock />}
          {config.caption && slide?.caption && <p className="frame-caption">{slide.caption}</p>}
        </div>
      )}
      {state.trouble && (
        <p className="frame-offline" role="status">
          Offline. Trying again…
        </p>
      )}
      {state.status === "loading" && !slide && !state.trouble && (
        <p className="frame-centered" role="status">
          Loading…
        </p>
      )}
      {state.status === "empty" && (
        <div className="frame-centered" role="status">
          <h1>No photos to show</h1>
          <p className="muted">Nothing here is ready to display yet.</p>
        </div>
      )}
      {menu && (
        <div
          className="frame-menu"
          role="dialog"
          aria-label="Photo frame menu"
          onClick={(e) => e.stopPropagation()}
        >
          <button type="button" ref={first} className="btn btn-glass" onClick={() => navigate("/frame?setup", { replace: true })}>
            Change what is shown
          </button>
          {fullscreenSupported() && (
            <button type="button" className="btn btn-glass" onClick={() => void (isFullscreen() ? exitFullscreen() : enterFullscreen())}>
              Full screen
            </button>
          )}
          <button
            type="button"
            className="btn btn-glass"
            onClick={() => {
              void exitFullscreen();
              navigate("/photos");
            }}
          >
            Exit the frame
          </button>
          <button type="button" className="btn btn-glass" onClick={closeMenu}>
            Close
          </button>
        </div>
      )}
      <div className="sr-only" role="status" aria-live="polite">
        {state.trouble ? "Offline. Trying again." : ""}
      </div>
    </div>
  );
}

/** The time and date, redrawn when the minute changes. */
function FrameClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const tick = () => {
      const d = new Date();
      setNow(d);
      timer = setTimeout(tick, 60_000 - (d.getSeconds() * 1000 + d.getMilliseconds()) + 50);
    };
    timer = setTimeout(tick, 60_000 - (now.getSeconds() * 1000 + now.getMilliseconds()) + 50);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className="frame-clock" aria-label="Time and date">
      <time dateTime={now.toISOString()} className="frame-time">
        {new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(now)}
      </time>
      <span className="frame-date">
        {new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric" }).format(now)}
      </span>
    </div>
  );
}
