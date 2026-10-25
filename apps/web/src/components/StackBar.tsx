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
  return (
    <section className="stack-bar" aria-label="Stack">
      <div className="stack-head">
        <span className="stack-count" aria-live="polite">
          {positionLabel(position, items.length)}
        </span>
        <span className="stack-step">
          <button
            type="button"
            className="btn btn-glass"
            aria-label="Previous photo in the stack"
            title="Previous in stack ["
            disabled={position === 0}
            onClick={() => stack.onStep(-1)}
          >
            ◂
          </button>
          <button
            type="button"
            className="btn btn-glass"
            aria-label="Next photo in the stack"
            title="Next in stack ]"
            disabled={position >= items.length - 1}
            onClick={() => stack.onStep(1)}
          >
            ▸
          </button>
        </span>
      </div>
      <ul className="stack-strip" ref={strip} aria-label="Photos in this stack">
        {items.map((it, i) => (
          <li key={it.id}>
            <button
              type="button"
              className={`stack-thumb${i === position ? " on" : ""}`}
              style={placeholderUrl(it.thumbhash) ? { backgroundImage: `url(${placeholderUrl(it.thumbhash)})` } : undefined}
              aria-label={`${it.original_name}${it.id === coverId ? ", cover" : ""}`}
              aria-current={i === position ? "true" : undefined}
              onClick={() => stack.onPick(it.id)}
            >
              {it.thumb_url && <img src={it.thumb_url} alt="" loading="lazy" draggable={false} />}
              {it.id === coverId && <span className="stack-cover-mark">Cover</span>}
            </button>
          </li>
        ))}
      </ul>
      <div className="stack-actions">
        <button type="button" className="btn btn-glass" disabled={busy || isCover} onClick={stack.onCover}>
          Set as stack cover
        </button>
        <button type="button" className="btn btn-glass" disabled={busy} onClick={stack.onRemove}>
          Remove from stack
        </button>
        <button type="button" className="btn btn-glass" disabled={busy} onClick={stack.onDissolve}>
          Unstack all
        </button>
      </div>
    </section>
  );
}
