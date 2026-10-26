import type { KeyboardEvent } from "react";
import type { PhotoFace } from "../api";
import "../viewer.css";

/** Where the displayed picture sits inside the zoomable layer (px): the regions are drawn in it, so they follow every zoom and pan. */
export interface PictureRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** A face the owner has named (a person with a name): its region opens that person; any other opens "Who is this?". */
export const namedFace = (f: PhotoFace) => !!f.person?.name?.trim();

/** What a screen reader says for a face on the photo. */
export const faceLabel = (f: PhotoFace) => (namedFace(f) ? `${f.person!.name!.trim()}: open their page` : "Unknown face: who is this?");

const pct = (v: number) => `${Math.round(v * 10000) / 100}%`;

/**
 * The faces of the photo on show as regions over the picture: each is a link (named) or a button (unclaimed) with the face's box,
 * reachable with the keyboard, outlined on hover and focus (or all the time with `showAll`). A tap or click is heard by the viewer's
 * zoom handler, not here (`data-tap`): the photo area owns every pointer, so that a press that pans or pinches never opens anything.
 * Enter and Space, and a click an assistive technology makes, activate here. The small "Not a face" button appears with the outline.
 */
export function FaceRegions({
  faces,
  rect,
  showAll,
  onOpen,
  onNotAFace,
}: {
  faces: PhotoFace[];
  rect: PictureRect;
  showAll: boolean;
  onOpen: (face: PhotoFace) => void;
  onNotAFace: (face: PhotoFace) => void;
}) {
  const onKey = (f: PhotoFace) => (e: KeyboardEvent) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    e.stopPropagation();
    onOpen(f);
  };
  return (
    <div className={`face-regions${showAll ? " all" : ""}`} style={{ left: rect.left, top: rect.top, width: rect.width, height: rect.height }}>
      {faces.map((f) => {
        const [x, y, w, h] = f.box;
        return (
          <span key={f.id} className={`face-spot${x + w / 2 > 0.5 ? " right" : ""}${y + h / 2 > 0.5 ? " low" : ""}`} style={{ left: pct(x), top: pct(y), width: pct(w), height: pct(h) }}>
            <span
              className="face-region"
              role={namedFace(f) ? "link" : "button"}
              tabIndex={0}
              aria-label={faceLabel(f)}
              data-tap=""
              data-face-id={f.id}
              onKeyDown={onKey(f)}
              onClick={(e) => {
                if (e.detail === 0) onOpen(f);
              }}
            />
            <button type="button" className="face-not" aria-label="Not a face" title="Not a face" onClick={() => onNotAFace(f)}>
              Not a face
            </button>
          </span>
        );
      })}
    </div>
  );
}
