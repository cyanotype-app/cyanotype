import type { RefObject } from "react";

/**
 * The zoom buttons over the photo's corner: out, the level (fit = 100 %), in, and Fit. The level text is written by
 * `usePhotoZoom` while a gesture runs, and said aloud (the hidden live region) once it has settled. On a touch screen the
 * group only shows while zoomed (pinching is how it is done there); the keys `+`, `-` and `z` work everywhere.
 */
export function ZoomControls({
  zoomed,
  onIn,
  onOut,
  onFit,
  levelRef,
  announceRef,
}: {
  zoomed: boolean;
  onIn: () => void;
  onOut: () => void;
  onFit: () => void;
  levelRef: RefObject<HTMLElement | null>;
  announceRef: RefObject<HTMLElement | null>;
}) {
  return (
    <div className={`zoom-controls${zoomed ? " on" : ""}`} role="group" aria-label="Zoom">
      <button type="button" className="btn btn-glass zoom-btn" aria-label="Zoom out" aria-keyshortcuts="-" onClick={onOut}>
        <span aria-hidden="true">−</span>
      </button>
      <span className="zoom-level" ref={levelRef} aria-hidden="true">
        100 %
      </span>
      <button type="button" className="btn btn-glass zoom-btn" aria-label="Zoom in" aria-keyshortcuts="+" onClick={onIn}>
        <span aria-hidden="true">+</span>
      </button>
      <button type="button" className="btn btn-glass zoom-btn zoom-fit" aria-label="Fit photo to screen" aria-keyshortcuts="z" onClick={onFit}>
        Fit
      </button>
      <span className="sr-only" aria-live="polite" ref={announceRef} />
    </div>
  );
}
