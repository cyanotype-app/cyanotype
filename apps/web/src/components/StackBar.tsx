import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useRef } from "react";
import type { AssetListItem } from "../api";
import { placeholderUrl } from "../thumbhash";
import { positionLabel } from "../stacks";

/** What the viewer needs to step through the stack of the photo on show and to change it. */
export interface StackView {
  /** The stack's photos in capture order. */
  items: AssetListItem[];
  /** The photo on show (an index into `items`). */
  position: number;
  coverId: string;
  /** The owner stacked these by hand (otherwise the worker found a burst). */
  manual: boolean;
  /** A change is being saved. */
  busy: boolean;
  onPick: (id: string) => void;
  /** Move through the stack (`[` and `]`); stops at both ends. */
  onStep: (delta: number) => void;
  onCover: () => void;
  onRemove: () => void;
  onDissolve: () => void;
}

/**
 * The strip under the viewer's photo while it is part of a stack: "Photo 2 of 5 in this stack", the stack's photos as a
 * filmstrip (the cover is marked) and the controls: set as cover, remove from the stack, unstack all. The viewer's own arrows
 * still move through the timeline; this strip (or `[` and `]`) moves through the stack.
 */
export function StackBar({ stack }: { stack: StackView }) {
  const { items, position, coverId, busy } = stack;
  const here = items[position];
  const strip = useRef<HTMLUListElement>(null);
  // Keep the photo on show in view when the strip scrolls (many photos, a narrow screen).
  useEffect(() => {
    const el = strip.current?.querySelector<HTMLElement>('[aria-current="true"]');
    el?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [position]);
  if (!here) return null;
  const isCover = here.id === coverId;
  const pill =
    "h-9 shrink-0 rounded-full bg-white/15 px-3.5 text-[0.8125rem] font-medium text-white hover:bg-white/25 disabled:pointer-events-none disabled:opacity-40";
  return (
    <section className="mx-auto flex w-full max-w-3xl flex-col gap-2 px-4 pb-3 text-white" aria-label="Stack">
      <div className="flex items-center gap-2">
        <span className="mr-auto text-sm text-white/80" aria-live="polite">
          {positionLabel(position, items.length)}
        </span>
        <span className="flex gap-1">
          <button
            type="button"
            className="grid size-9 grid-cols-1 place-items-center rounded-full bg-white/15 hover:bg-white/25 disabled:pointer-events-none disabled:opacity-40"
            aria-label="Previous photo in the stack"
            title="Previous in stack ["
            disabled={position === 0}
            onClick={() => stack.onStep(-1)}
          >
            <ChevronLeft className="size-5" aria-hidden="true" />
          </button>
          <button
            type="button"
            className="grid size-9 grid-cols-1 place-items-center rounded-full bg-white/15 hover:bg-white/25 disabled:pointer-events-none disabled:opacity-40"
            aria-label="Next photo in the stack"
            title="Next in stack ]"
            disabled={position >= items.length - 1}
            onClick={() => stack.onStep(1)}
          >
            <ChevronRight className="size-5" aria-hidden="true" />
          </button>
        </span>
      </div>
      <ul className="m-0 flex list-none gap-1.5 overflow-x-auto p-0 pb-1" ref={strip} aria-label="Photos in this stack">
        {items.map((it, i) => (
          <li key={it.id} className="shrink-0">
            <button
              type="button"
              className={`relative block size-14 overflow-hidden rounded-lg bg-white/10 bg-cover bg-center ring-2 transition-[box-shadow,opacity] ${
                i === position ? "opacity-100 ring-white" : "opacity-70 ring-transparent hover:opacity-100"
              }`}
              style={placeholderUrl(it.thumbhash) ? { backgroundImage: `url(${placeholderUrl(it.thumbhash)})` } : undefined}
              aria-label={`${it.original_name}${it.id === coverId ? ", cover" : ""}`}
              aria-current={i === position ? "true" : undefined}
              onClick={() => stack.onPick(it.id)}
            >
              {it.thumb_url && <img src={it.thumb_url} alt="" loading="lazy" draggable={false} className="absolute inset-0 size-full object-cover" />}
              {it.id === coverId && (
                <span className="absolute inset-x-0 bottom-0 bg-black/60 py-px text-center text-[0.5625rem] font-semibold tracking-wide text-white uppercase">
                  Cover
                </span>
              )}
            </button>
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={pill} disabled={busy || isCover} onClick={stack.onCover}>
          Set as stack cover
        </button>
        <button type="button" className={pill} disabled={busy} onClick={stack.onRemove}>
          Remove from stack
        </button>
        <button type="button" className={pill} disabled={busy} onClick={stack.onDissolve}>
          Unstack all
        </button>
      </div>
    </section>
  );
}
