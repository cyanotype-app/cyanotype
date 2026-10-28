import type { FaceBox } from "../api";
import { KIND_TITLE } from "../petsText";
import { cn } from "../ui/cn";

/**
 * A photo with an outline round each animal found in it, to tap the one that is meant. The wrapper is exactly as big as the picture, so
 * the boxes (fractions of the preview) sit on the animals at any size; each box is a button (numbered, so a keyboard or screen reader can
 * tell them apart) with a hit area bigger than a small animal. Choosing is `aria-pressed`: the caller keeps the answer.
 */
export function AnimalPicker({
  url,
  candidates,
  value,
  onChange,
  showKinds = false,
  label = "The photo, with each animal outlined",
}: {
  url: string;
  candidates: readonly { detection_id: string; box: FaceBox; kind?: "dog" | "cat" }[];
  /** The chosen animal's id. */
  value: string | null;
  onChange: (detectionId: string) => void;
  /** Say what each animal is called ("1 Cat"), for when the kinds differ from what the owner means. */
  showKinds?: boolean;
  label?: string;
}) {
  const pct = (v: number) => `${Math.round(v * 10000) / 100}%`;
  return (
    <div className="mb-1 flex justify-center">
      <span className="relative inline-block max-w-full overflow-hidden rounded-card bg-surface leading-[0]" data-testid="animal-picker">
        <img src={url} alt={label} draggable={false} className="block h-auto max-h-[min(50vh,22rem)] w-auto max-w-full" />
        {candidates.map((c, i) => {
          const [x, y, w, h] = c.box;
          const on = value === c.detection_id;
          return (
            <button
              key={c.detection_id}
              type="button"
              aria-pressed={on}
              aria-label={`Animal ${i + 1} of ${candidates.length}${c.kind ? `, ${KIND_TITLE[c.kind].toLowerCase()}` : ""}`}
              data-box=""
              onClick={() => onChange(c.detection_id)}
              className={cn(
                "absolute box-border rounded-[6px] border-2 transition-[background-color,border-color,box-shadow] duration-150 ease-soft",
                "before:absolute before:-inset-2 before:content-['']",
                "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-white",
                on ? "border-white bg-accent/35 shadow-[0_0_0_2px_var(--color-accent)]" : "border-white/85 bg-black/10 shadow-[0_0_0_1px_rgb(0_0_0/0.55)] hover:bg-white/20",
              )}
              style={{ left: pct(x), top: pct(y), width: pct(w), height: pct(h) }}
            >
              <span
                aria-hidden="true"
                className={cn(
                  "absolute top-1 left-1 flex h-5 min-w-5 items-center justify-center rounded-full text-[0.6875rem] leading-none font-semibold",
                  showKinds && c.kind ? "gap-1 px-1.5" : "",
                  on ? "bg-accent text-on-accent" : "bg-black/60 text-white",
                )}
              >
                {i + 1}
                {showKinds && c.kind ? ` ${KIND_TITLE[c.kind]}` : ""}
              </span>
            </button>
          );
        })}
      </span>
    </div>
  );
}
