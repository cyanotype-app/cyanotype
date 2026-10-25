import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { useAuthOptional } from "../auth";
import { SlideshowController } from "./controller";
import type { SlideSource } from "./deck";
import { exitFullscreen, isFullscreen, prefersReducedMotion, useFullscreen, useReducedMotion, useWakeLock } from "./device";
import { CollapseIcon, ExpandIcon, GearIcon, NextIcon, PauseIcon, PlayIcon, PrevIcon } from "./icons";
import { makeLoader } from "./loader";
import { PreviewCache } from "./previews";
import { DURATIONS, ORDERS, loadSettings, saveSettings, type SlideshowSettings } from "./settings";
import { SlideStage } from "./SlideStage";
import { useShowState } from "./useShow";

/** Controls hide after this long without the pointer, a touch or a key. */
const IDLE_MS = 3000;
/** A horizontal swipe longer than this (px) goes to the next or previous photo. */
const SWIPE_PX = 60;

/**
 * The fullscreen slideshow: a dark layer over the app that shows the source's photos one at a time. The controller does the
 * timing and loading; this draws its state, shows the control bar on movement and hides it again, and maps the keys:
 * Space play/pause, ← → previous/next, F full screen, Esc close.
 */
export function Slideshow({ source, onClose }: { source: SlideSource; onClose: () => void }) {
  const [settings, setSettings] = useState<SlideshowSettings>(loadSettings);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const [ctl, setCtl] = useState<SlideshowController | null>(null);
  const reduced = useReducedMotion();
  const auth = useAuthOptional();
  const state = useShowState(ctl);
  const fs = useFullscreen();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [shown, setShown] = useState(true);
  const root = useRef<HTMLDivElement>(null);
  const bars = useRef<HTMLDivElement>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const settingsOpenRef = useRef(false);
  settingsOpenRef.current = settingsOpen;
  const swipe = useRef<{ x: number; y: number } | null>(null);
  const swiped = useRef(false);
  const cache = useMemo(() => new PreviewCache(), []);

  // The controller lives as long as the source: created in the effect so a re-mount (StrictMode) gets a fresh one.
  useEffect(() => {
    const s = settingsRef.current;
    const c = new SlideshowController({
      source,
      order: s.order,
      loop: s.loop,
      durationMs: s.duration * 1000,
      transition: s.transition,
      load: makeLoader(source, cache),
      reducedMotion: prefersReducedMotion,
      offline: () => typeof navigator !== "undefined" && navigator.onLine === false,
    });
    setCtl(c);
    c.start();
    const online = () => c.retryNow();
    window.addEventListener("online", online);
    return () => {
      window.removeEventListener("online", online);
      c.destroy();
      cache.clear();
      setCtl(null);
    };
  }, [source, cache]);

  useEffect(() => {
    ctl?.setOptions({ order: settings.order, loop: settings.loop, durationMs: settings.duration * 1000, transition: settings.transition });
    saveSettings(settings);
  }, [ctl, settings]);

  useWakeLock(true);

  // Focus in, page scroll off; both undone on the way out. A slideshow that took the browser full screen gives it back.
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    root.current?.focus();
    return () => {
      document.body.style.overflow = overflow;
      if (isFullscreen()) void exitFullscreen();
      opener?.focus?.();
    };
  }, []);

  // Controls: shown on movement, hidden after a quiet spell (never while paused with the panel open or a key is on a button).
  const reveal = useCallback(() => {
    setShown(true);
    if (hideTimer.current) clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(function hide() {
      const focused = bars.current?.querySelector(":focus-visible");
      if (settingsOpenRef.current || focused) {
        hideTimer.current = setTimeout(hide, IDLE_MS);
        return;
      }
      setShown(false);
    }, IDLE_MS);
  }, []);
  useEffect(() => {
    reveal();
    return () => {
      if (hideTimer.current) clearTimeout(hideTimer.current);
    };
  }, [reveal]);

  const close = useCallback(() => onClose(), [onClose]);

  // Keys. In the capture phase on the window, and every key is kept from the page underneath (the viewer's own shortcuts
  // must not act on a slideshow that was started from it).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      e.stopImmediatePropagation();
      reveal();
      const t = e.target as HTMLElement | null;
      const onControl = !!t?.closest("button, a, input, select, textarea");
      const handled = (fn: () => void) => {
        e.preventDefault();
        if (!e.repeat) fn();
      };
      switch (e.key) {
        case "Escape":
          handled(() => (settingsOpenRef.current ? setSettingsOpen(false) : close()));
          break;
        case " ":
          // A button or a field keeps its own use of Space.
          if (onControl) return;
          handled(() => ctl?.toggle());
          break;
        case "ArrowRight":
        case "ArrowLeft":
          if (t?.closest("input, select, textarea")) return;
          e.preventDefault();
          if (e.key === "ArrowRight") ctl?.next();
          else ctl?.prev();
          break;
        case "f":
        case "F":
          if (fs.supported) handled(fs.toggle);
          break;
        case "Tab": {
          // Stay inside the slideshow.
          const f = Array.from(root.current?.querySelectorAll<HTMLElement>("button:not([disabled]), select, input") ?? []).filter(
            (el) => el.offsetParent !== null || el === document.activeElement,
          );
          const first = f[0];
          const last = f[f.length - 1];
          if (!first || !last) return;
          if (e.shiftKey && (document.activeElement === first || document.activeElement === root.current)) {
            e.preventDefault();
            last.focus();
          } else if (!e.shiftKey && document.activeElement === last) {
            e.preventDefault();
            first.focus();
          }
          break;
        }
      }
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [ctl, close, fs.supported, fs.toggle, reveal]);

  const onPointerMove = (e: ReactPointerEvent) => {
    // A touch is a tap or a swipe, handled below; only a mouse or pen moving reveals the controls.
    if (e.pointerType !== "touch") reveal();
  };
  const onStageDown = (e: ReactPointerEvent) => {
    swiped.current = false;
    swipe.current = e.pointerType === "touch" ? { x: e.clientX, y: e.clientY } : null;
  };
  const onStageUp = (e: ReactPointerEvent) => {
    const s = swipe.current;
    swipe.current = null;
    if (!s) return;
    const dx = e.clientX - s.x;
    if (Math.abs(dx) > SWIPE_PX && Math.abs(e.clientY - s.y) < Math.abs(dx) / 2) {
      swiped.current = true;
      if (dx < 0) ctl?.next();
      else ctl?.prev();
    }
  };
  const onStageClick = () => {
    if (swiped.current) {
      swiped.current = false;
      return;
    }
    reveal();
  };

  const playing = state.status === "playing";
  const idle = !shown && playing;
  const { slide } = state;
  const counter = state.position > 0 ? `${state.position}${state.total ? ` / ${state.total}` : ""}` : "";

  return (
    <div
      ref={root}
      className={`slideshow${idle ? " idle" : ""}`}
      role="dialog"
      aria-modal="true"
      aria-label={`Slideshow: ${source.title}`}
      tabIndex={-1}
      onPointerMove={onPointerMove}
    >
      <div className="slideshow-stage" onPointerDown={onStageDown} onPointerUp={onStageUp} onClick={onStageClick}>
        <SlideStage slide={slide} outgoing={state.outgoing} fade={state.fade} />
      </div>
      <div className="sr-only" role="status" aria-live="polite">
        {state.announce}
      </div>

      <div className="slideshow-bar slideshow-top" ref={bars}>
        <button type="button" className="btn btn-glass" onClick={close} aria-keyshortcuts="Escape">
          Close
        </button>
        <span className="slideshow-count" aria-hidden="true">
          {counter}
        </span>
        {fs.supported ? (
          <button
            type="button"
            className="btn btn-glass slideshow-fs"
            aria-pressed={fs.active}
            aria-keyshortcuts="f"
            aria-label={fs.active ? "Exit full screen" : "Full screen"}
            onClick={fs.toggle}
          >
            {fs.active ? <CollapseIcon /> : <ExpandIcon />}
          </button>
        ) : (
          <span />
        )}
      </div>
      <div className="slideshow-bar slideshow-bottom">
        <div className="slideshow-transport">
          <button type="button" className="btn btn-glass" aria-label="Previous photo" aria-keyshortcuts="ArrowLeft" onClick={() => ctl?.prev()}>
            <PrevIcon />
          </button>
          <button
            type="button"
            className="btn btn-glass slideshow-play"
            aria-label={playing ? "Pause" : "Play"}
            aria-keyshortcuts="Space"
            onClick={() => ctl?.toggle()}
          >
            {playing ? <PauseIcon /> : <PlayIcon />}
          </button>
          <button type="button" className="btn btn-glass" aria-label="Next photo" aria-keyshortcuts="ArrowRight" onClick={() => ctl?.next()}>
            <NextIcon />
          </button>
        </div>
        <button
          type="button"
          className={`btn btn-glass slideshow-gear${settingsOpen ? " on" : ""}`}
          aria-label="Slideshow settings"
          aria-expanded={settingsOpen}
          aria-controls="slideshow-settings"
          onClick={() => setSettingsOpen((o) => !o)}
        >
          <GearIcon />
        </button>
      </div>

      {settingsOpen && (
        <div id="slideshow-settings" className="slideshow-settings" role="group" aria-label="Slideshow settings">
          <label>
            <span>Time per photo</span>
            <select value={settings.duration} onChange={(e) => setSettings({ ...settings, duration: Number(e.target.value) })}>
              {DURATIONS.map((d) => (
                <option key={d} value={d}>
                  {d} seconds
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Order</span>
            <select value={settings.order} onChange={(e) => setSettings({ ...settings, order: e.target.value as SlideshowSettings["order"] })}>
              {ORDERS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Transition</span>
            <select
              value={reduced ? "none" : settings.transition}
              disabled={reduced}
              onChange={(e) => setSettings({ ...settings, transition: e.target.value as SlideshowSettings["transition"] })}
            >
              <option value="crossfade">Crossfade</option>
              <option value="none">None</option>
            </select>
          </label>
          {reduced && <p className="muted small">Your device asks for less motion, so photos change without a fade.</p>}
          <label className="slideshow-check">
            <input type="checkbox" checked={settings.loop} onChange={(e) => setSettings({ ...settings, loop: e.target.checked })} />
            <span>Start again at the end</span>
          </label>
        </div>
      )}

      {state.trouble && (
        <p className="slideshow-note" role="status">
          Can’t reach the server. Trying again…
        </p>
      )}
      {state.status === "loading" && !slide && !state.trouble && (
        <p className="slideshow-note slideshow-centered" role="status">
          Loading…
        </p>
      )}
      {state.status === "empty" && (
        <div className="slideshow-card" role="status">
          <h2>No photos to show</h2>
          <p className="muted">There is nothing here yet, or none of it is ready to display.</p>
          <button type="button" className="btn btn-primary" onClick={close}>
            Close
          </button>
        </div>
      )}
      {state.status === "ended" && (
        <div className="slideshow-card" role="status">
          <h2>That’s all</h2>
          <div className="slideshow-card-actions">
            <button type="button" className="btn btn-primary" onClick={() => ctl?.play()}>
              Play again
            </button>
            <button type="button" className="btn btn-glass" onClick={close}>
              Close
            </button>
          </div>
        </div>
      )}
      {state.status === "signed-out" && (
        <div className="slideshow-card" role="alert">
          <h2>{source.expired?.title ?? "Sign in again"}</h2>
          <p className="muted">{source.expired?.text ?? "Your session has ended."}</p>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => {
              close();
              if (!source.expired) auth?.setAnonymous();
            }}
          >
            {source.expired ? "Close" : "Sign in"}
          </button>
        </div>
      )}
    </div>
  );
}
