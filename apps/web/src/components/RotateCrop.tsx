import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import {
  dragCrop,
  fitRatio,
  frameSize,
  handleLabel,
  isFullCrop,
  isIdentity,
  MAX_STRAIGHTEN,
  RATIOS,
  ratioOf,
  recipeText,
  sameRecipe,
  straightenZoom,
  turn,
  type Crop,
  type Handle,
  type Recipe,
} from "../recipeText";

interface Props {
  /** The photo's name, for the labels. */
  name: string;
  /** The whole upright picture to work on (not the cropped one), at preview size. */
  src: string;
  /** The recipe the photo has now. */
  initial: Recipe;
  /** The file's size in pixels (as displayed), when known: the editor then says how large the result is. */
  fileSize?: [number, number] | null;
  /** Start with the crop frame showing (the `c` key opened the editor). */
  startCropping?: boolean;
  /** Turn it right away once (the `r` key opened the editor). */
  startTurned?: boolean;
  /** Save the recipe. Rejecting keeps the editor open with the error shown. */
  onSave: (recipe: Recipe) => Promise<void>;
  /** Put the photo back to the file's picture. Shown while the photo has a recipe. */
  onRevert: () => Promise<void>;
  onCancel: () => void;
}

const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
/** A keyboard nudge of a handle, as a fraction of the frame (Shift: five times that). */
const NUDGE = 0.01;
/** Used where nothing can be measured (a test): the room the frame fits into. */
const FALLBACK_ROOM: [number, number] = [320, 240];

/**
 * The rotate / straighten / crop editor, a full-screen dialog over the viewer. It shows the whole upright picture turned and
 * zoomed by the recipe the way the worker renders it, with a crop frame to drag by its handles (pointer and touch) or with the
 * arrow keys. Keys: `r` rotates right (Shift+R left), `c` shows or hides the crop handles, Enter saves, Escape cancels.
 */
export function RotateCrop({ name, src, initial, fileSize, startCropping = false, startTurned = false, onSave, onRevert, onCancel }: Props) {
  const [recipe, setRecipe] = useState<Recipe>(() => {
    const start = startTurned ? turn(initial, 1) : initial;
    // Opened with `c`: a frame inside the picture, so its handles can be reached on a phone.
    return startCropping && !start.crop ? { ...start, crop: { x: 0.05, y: 0.05, w: 0.9, h: 0.9 } } : start;
  });
  const [cropping, setCropping] = useState(startCropping);
  const [ratioId, setRatioId] = useState("free");
  const [natural, setNatural] = useState<[number, number] | null>(null);
  const [room, setRoom] = useState<[number, number] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{ handle: Handle; x: number; y: number; start: Crop; pointer: number } | null>(null);

  // The picture's shape: from the file when known, else from the image itself once it has loaded.
  const shape: [number, number] | null = fileSize ?? natural;
  const [fw, fh] = shape ? frameSize(recipe, shape[0], shape[1]) : [4, 3];
  const ratioChoice = RATIOS.find((r) => r.id === ratioId) ?? RATIOS[0]!;
  const ratio = ratioOf(ratioChoice, fw, fh);

  // How big the frame is drawn: as large as fits the stage, in the shape of the turned picture.
  useLayoutEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      // The padding keeps the handles (44 px targets centred on the edge) inside the stage.
      setRoom(r.width > 0 && r.height > 0 ? [Math.max(40, r.width - 48), Math.max(40, r.height - 48)] : null);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const [roomW, roomH] = room ?? FALLBACK_ROOM;
  const scale = Math.min(roomW / fw, roomH / fh);
  const bw = Math.max(1, Math.floor(fw * scale));
  const bh = Math.max(1, Math.floor(fh * scale));

  const changed = !sameRecipe(recipe, initial);
  const hasSaved = !isIdentity(initial);
  const crop = recipe.crop && !isFullCrop(recipe.crop) ? recipe.crop : null;
  const showFrame = cropping || crop !== null;
  const frameCrop: Crop = recipe.crop ?? { x: 0, y: 0, w: 1, h: 1 };

  function rotate(quarters: number) {
    setRecipe((r) => turn(r, quarters));
    // A fixed ratio turns with the picture (a 3:2 crop of the right-turned picture is a 2:3 one).
    if (ratioChoice.ratio !== null && ratioChoice.ratio !== "original") {
      const flipped = 1 / ratioChoice.ratio;
      setRatioId(RATIOS.find((r) => typeof r.ratio === "number" && Math.abs(r.ratio - flipped) < 1e-6)?.id ?? "free");
    }
  }
  function toggleCrop() {
    setCropping((on) => {
      if (!on && !recipe.crop) setRecipe((r) => ({ ...r, crop: { x: 0.05, y: 0.05, w: 0.9, h: 0.9 } }));
      return !on;
    });
  }
  function chooseRatio(id: string) {
    const choice = RATIOS.find((r) => r.id === id)!;
    setRatioId(id);
    setCropping(true);
    const r = ratioOf(choice, fw, fh);
    if (r !== null) setRecipe((cur) => ({ ...cur, crop: fitRatio(cur.crop, r, fw, fh) }));
    else if (!recipe.crop) setRecipe((cur) => ({ ...cur, crop: { x: 0.05, y: 0.05, w: 0.9, h: 0.9 } }));
  }
  function reset() {
    setRecipe(initial);
    setRatioId("free");
    setCropping(false);
    setError(null);
  }
  async function save() {
    if (busy || !changed) return;
    setBusy(true);
    setError(null);
    try {
      await onSave(recipe);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not save the changes.");
      setBusy(false);
    }
  }
  async function revert() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await onRevert();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not go back to the original.");
      setBusy(false);
    }
  }

  // Keyboard: r / Shift+R turn, c crops, Enter saves, Escape cancels; Tab stays inside.
  const latest = useRef({ rotate, toggleCrop, save, onCancel });
  useEffect(() => {
    latest.current = { rotate, toggleCrop, save, onCancel };
  });
  useEffect(() => {
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target as HTMLElement | null;
      const key = e.key;
      if (key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        latest.current.onCancel();
      } else if (key === "Enter") {
        // Enter on a button presses it; anywhere else it saves.
        if (target?.closest("button, a[href]")) return;
        e.preventDefault();
        void latest.current.save();
      } else if ((key === "r" || key === "R") && !e.repeat) {
        e.preventDefault();
        latest.current.rotate(e.shiftKey || key === "R" ? -1 : 1);
      } else if ((key === "c" || key === "C") && !e.repeat) {
        e.preventDefault();
        latest.current.toggleCrop();
      } else if (key === "Tab" && rootRef.current) {
        const f = Array.from(rootRef.current.querySelectorAll<HTMLElement>("button:not(:disabled), input, [tabindex='0']"));
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
    // Capture, so the viewer under the editor never sees the key.
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, []);
  // Focus goes to the dialog itself, so Enter saves and the keys work at once; Tab then reaches the buttons.
  useEffect(() => {
    rootRef.current?.focus();
  }, []);

  function down(handle: Handle, e: PointerEvent<HTMLElement>) {
    if (e.button !== 0 && e.pointerType === "mouse") return;
    e.preventDefault();
    e.stopPropagation();
    try {
      // Keeps the drag with this handle when the finger or the mouse leaves it.
      e.currentTarget.setPointerCapture?.(e.pointerId);
    } catch {
      /* a pointer that is already gone: the drag is just not captured */
    }
    drag.current = { handle, x: e.clientX, y: e.clientY, start: frameCrop, pointer: e.pointerId };
  }
  function move(e: PointerEvent<HTMLElement>) {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    const next = dragCrop(d.start, d.handle, (e.clientX - d.x) / bw, (e.clientY - d.y) / bh, { fw, fh, ratio });
    setRecipe((r) => ({ ...r, crop: next }));
  }
  function up(e: PointerEvent<HTMLElement>) {
    if (drag.current?.pointer === e.pointerId) drag.current = null;
  }
  function nudge(handle: Handle, e: KeyboardEvent<HTMLElement>) {
    const step = (e.shiftKey ? 5 : 1) * NUDGE;
    const d: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, -step],
      ArrowDown: [0, step],
    };
    const delta = d[e.key];
    if (!delta) return;
    e.preventDefault();
    e.stopPropagation();
    setRecipe((r) => ({ ...r, crop: dragCrop(r.crop ?? frameCrop, handle, delta[0], delta[1], { fw, fh, ratio }) }));
  }

  const turned = recipe.rotate % 180 === 90;
  const zoom = straightenZoom(recipe.straighten, fw, fh);
  const imgStyle = {
    width: turned ? bh : bw,
    height: turned ? bw : bh,
    transform: `translate(-50%, -50%) rotate(${recipe.rotate + recipe.straighten}deg) scale(${zoom})`,
  };
  const boxStyle = (c: Crop) => ({ left: c.x * bw, top: c.y * bh, width: c.w * bw, height: c.h * bh });

  return (
    <div className="rc" role="dialog" aria-modal="true" aria-label="Rotate and crop" tabIndex={-1} ref={rootRef}>
      <div className="rc-bar">
        <button type="button" className="btn btn-glass" onClick={onCancel} aria-keyshortcuts="Escape">
          Cancel
        </button>
        <span className="rc-title">{name}</span>
        <button type="button" className="btn btn-glass rc-save" onClick={() => void save()} disabled={busy || !changed} aria-keyshortcuts="Enter">
          {busy ? "Saving…" : "Save"}
        </button>
      </div>
      <div className="rc-stage" ref={stageRef}>
        <div className="rc-frame-wrap" style={{ width: bw, height: bh }}>
          <div className="rc-frame">
            <img src={src} alt="" draggable={false} style={imgStyle} onLoad={(e) => setNatural([e.currentTarget.naturalWidth, e.currentTarget.naturalHeight])} />
            {showFrame && <div className="rc-dim" style={boxStyle(frameCrop)} />}
          </div>
          {showFrame && (
            <div
              className={`rc-crop${cropping ? " active" : ""}`}
              style={boxStyle(frameCrop)}
              data-testid="crop-box"
              {...(cropping
                ? {
                    role: "group",
                    tabIndex: 0,
                    "aria-label": `${handleLabel("move")}, move with the arrow keys`,
                    onPointerDown: (e: PointerEvent<HTMLElement>) => down("move", e),
                    onPointerMove: move,
                    onPointerUp: up,
                    onPointerCancel: up,
                    onKeyDown: (e: KeyboardEvent<HTMLElement>) => nudge("move", e),
                  }
                : {})}
            >
              {cropping &&
                HANDLES.map((h) => (
                  <button
                    key={h}
                    type="button"
                    className={`rc-handle rc-${h}`}
                    aria-label={handleLabel(h)}
                    onPointerDown={(e) => down(h, e)}
                    onPointerMove={move}
                    onPointerUp={up}
                    onPointerCancel={up}
                    onKeyDown={(e) => nudge(h, e)}
                  />
                ))}
            </div>
          )}
        </div>
      </div>
      <div className="rc-tools">
        <p className="rc-summary" aria-live="polite">
          {recipeText(recipe, shape?.[0], shape?.[1])}
        </p>
        <div className="rc-row">
          <button type="button" className="btn btn-glass" onClick={() => rotate(-1)} aria-label="Rotate left" aria-keyshortcuts="Shift+R">
            <TurnIcon flip />
            <span className="rc-label">Left</span>
          </button>
          <button type="button" className="btn btn-glass" onClick={() => rotate(1)} aria-label="Rotate right" aria-keyshortcuts="r">
            <TurnIcon />
            <span className="rc-label">Right</span>
          </button>
          <button type="button" className={`btn btn-glass${cropping ? " on" : ""}`} onClick={toggleCrop} aria-pressed={cropping} aria-keyshortcuts="c">
            <CropIcon />
            <span className="rc-label">Crop</span>
          </button>
          <button type="button" className="btn btn-glass" onClick={reset} disabled={!changed || busy}>
            Reset
          </button>
          {hasSaved && (
            <button type="button" className="btn btn-glass" onClick={() => void revert()} disabled={busy}>
              Revert to original
            </button>
          )}
        </div>
        <label className="rc-straighten">
          <span>Straighten</span>
          <input
            type="range"
            min={-MAX_STRAIGHTEN}
            max={MAX_STRAIGHTEN}
            step={0.1}
            value={recipe.straighten}
            onChange={(e) => setRecipe((r) => ({ ...r, straighten: Number(e.target.value) }))}
            aria-valuetext={`${recipe.straighten.toFixed(1)} degrees`}
          />
          <output>{recipe.straighten.toFixed(1)}°</output>
        </label>
        {cropping && (
          <div className="rc-ratios" role="group" aria-label="Crop shape">
            {RATIOS.map((r) => (
              <button key={r.id} type="button" className={`btn btn-glass rc-ratio${ratioId === r.id ? " on" : ""}`} aria-pressed={ratioId === r.id} onClick={() => chooseRatio(r.id)}>
                {r.label}
              </button>
            ))}
          </div>
        )}
        {error && (
          <p className="rc-error" role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}

/** A quarter turn arrow; `flip` mirrors it for left. */
function TurnIcon({ flip = false }: { flip?: boolean }) {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" style={flip ? { transform: "scaleX(-1)" } : undefined}>
      <path d="M20 12a8 8 0 1 1-2.6-5.9" />
      <path d="M20 4v5h-5" />
    </svg>
  );
}

export function CropIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M6 2v14a2 2 0 0 0 2 2h14" />
      <path d="M18 22V8a2 2 0 0 0-2-2H2" />
    </svg>
  );
}
