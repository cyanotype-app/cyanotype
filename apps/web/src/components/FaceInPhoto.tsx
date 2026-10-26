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
    <div className="face-context flex justify-center">
      <span className="face-context-frame relative inline-block max-w-full overflow-hidden rounded-card bg-surface leading-[0]">
        <img src={url} alt={label} draggable={false} className="block h-auto max-h-[45dvh] max-w-full" />
        <span
          className="pointer-events-none absolute rounded-[3px] border-2 border-white shadow-[0_0_0_100vmax_rgb(0_0_0/0.45)]"
          data-testid="face-context-box"
          aria-hidden="true"
          style={{ left: pct(x), top: pct(y), width: pct(w), height: pct(h) }}
        />
      </span>
    </div>
  );
}
