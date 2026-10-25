import type { FaceBox } from "../api";

/**
 * The whole photo (its preview) with one face outlined and the rest dimmed, so it is clear which face of a group photo a question is
 * about. The wrapper is exactly as big as the picture (it shrinks to it), so the box, in fractions of the preview, sits on the face at
 * any size. The picture never grows past `max-height`: it is a glance, not the viewer.
 */
export function FaceInPhoto({ url, box, label = "The photo, with the face in question outlined" }: { url: string; box: FaceBox; label?: string }) {
  const [x, y, w, h] = box;
  const pct = (v: number) => `${Math.round(v * 10000) / 100}%`;
  return (
    <div className="face-context">
      <span className="face-context-frame">
        <img src={url} alt={label} draggable={false} />
        <span className="face-context-box" data-testid="face-context-box" aria-hidden="true" style={{ left: pct(x), top: pct(y), width: pct(w), height: pct(h) }} />
      </span>
    </div>
  );
}
