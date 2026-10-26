import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useRef } from "react";
import type { AssetListItem } from "../api";
import { placeholderUrl } from "../thumbhash";
import { positionLabel } from "../stacks";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";

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
  return (
    <section className="grid grid-cols-1 max-w-full gap-2 rounded-card bg-black/45 p-2.5 text-white backdrop-blur-md" aria-label="Stack">
      <div className="flex items-center justify-between gap-2 pl-1 text-sm">
        <span aria-live="polite">{positionLabel(position, items.length)}</span>
        <span className="inline-flex gap-1">
          <Button
            variant="overlay"
            size="icon-sm"
            aria-label="Previous photo in the stack"
            title="Previous in stack ["
            disabled={position === 0}
            onClick={() => stack.onStep(-1)}
          >
            <ChevronLeft aria-hidden="true" />
          </Button>
          <Button
            variant="overlay"
            size="icon-sm"
            aria-label="Next photo in the stack"
            title="Next in stack ]"
            disabled={position >= items.length - 1}
            onClick={() => stack.onStep(1)}
          >
            <ChevronRight aria-hidden="true" />
          </Button>
        </span>
      </div>
      <ul className="m-0 flex list-none gap-1.5 overflow-x-auto p-0.5 [scrollbar-width:thin]" ref={strip} aria-label="Photos in this stack">
        {items.map((it, i) => (
          <li key={it.id} className="flex-none">
            <button
              type="button"
              className={cn(
                "relative block size-12 overflow-hidden rounded-lg border-2 bg-white/10 bg-cover p-0 transition-[border-color,transform] duration-150",
                i === position ? "scale-100 border-white" : "scale-[0.92] border-transparent",
              )}
              style={placeholderUrl(it.thumbhash) ? { backgroundImage: `url(${placeholderUrl(it.thumbhash)})` } : undefined}
              aria-label={`${it.original_name}${it.id === coverId ? ", cover" : ""}`}
              aria-current={i === position ? "true" : undefined}
              onClick={() => stack.onPick(it.id)}
            >
              {it.thumb_url && <img src={it.thumb_url} alt="" loading="lazy" draggable={false} className="block size-full object-cover" />}
              {it.id === coverId && <span className="absolute inset-x-0 bottom-0 bg-black/65 text-[0.6rem] leading-[1.3] font-bold">Cover</span>}
            </button>
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap gap-1.5">
        <Button variant="overlay" size="sm" disabled={busy || isCover} onClick={stack.onCover}>
          Set as stack cover
        </Button>
        <Button variant="overlay" size="sm" disabled={busy} onClick={stack.onRemove}>
          Remove from stack
        </Button>
        <Button variant="overlay" size="sm" disabled={busy} onClick={stack.onDissolve}>
          Unstack all
        </Button>
      </div>
    </section>
  );
}
