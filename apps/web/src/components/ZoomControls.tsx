import { Maximize, Minus, Plus } from "lucide-react";
import type { RefObject } from "react";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";

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
  className,
}: {
  zoomed: boolean;
  onIn: () => void;
  onOut: () => void;
  onFit: () => void;
  levelRef: RefObject<HTMLElement | null>;
  announceRef: RefObject<HTMLElement | null>;
  /** Where it sits (the corner of the photo's area by default). */
  className?: string;
}) {
  return (
    <div
      className={cn(
        "absolute right-3 bottom-3 z-[2] flex items-center gap-0.5 rounded-full bg-black/45 p-1 text-white backdrop-blur-md",
        !zoomed && "pointer-coarse:hidden",
        className,
      )}
      role="group"
      aria-label="Zoom"
    >
      <Button variant="overlay" size="icon" className="bg-transparent" aria-label="Zoom out" aria-keyshortcuts="-" onClick={onOut}>
        <Minus aria-hidden="true" />
      </Button>
      <span className="min-w-[3.25rem] text-center text-[0.8125rem] tabular-nums" ref={levelRef} aria-hidden="true" data-zoom-level="">
        100 %
      </span>
      <Button variant="overlay" size="icon" className="bg-transparent" aria-label="Zoom in" aria-keyshortcuts="+" onClick={onIn}>
        <Plus aria-hidden="true" />
      </Button>
      <Button variant="overlay" size="icon" className="bg-transparent" aria-label="Fit photo to screen" aria-keyshortcuts="z" onClick={onFit}>
        <Maximize aria-hidden="true" />
      </Button>
      <span className="sr-only" aria-live="polite" ref={announceRef} />
    </div>
  );
}
