import { useState } from "react";
import type { FaceBox } from "../api";
import { cropStyle } from "../peopleText";
import { cn } from "../ui/cn";

/**
 * A face, cropped out of its photo's preview with CSS: the preview is drawn big inside a square, overflow-hidden box so only the
 * face shows. No new file is made; the boxes are fractions of the preview, so its size on the wire is read from the loaded picture.
 * Decorative (`alt=""`): whatever holds it (a link, a button) carries the name. It fills its width: give it a size (`className`).
 * (`face-crop` and `round` stay as names, for the layout checks and the tests.)
 */
export function FaceCrop({ url, box, round = true, className = "" }: { url: string; box: FaceBox; round?: boolean; className?: string }) {
  const [size, setSize] = useState<[number, number] | null>(null);
  const style = size ? cropStyle(box, size[0], size[1]) : null;
  return (
    <span
      className={cn("face-crop relative block aspect-square w-full overflow-hidden bg-surface", round && "round rounded-full", className)}
      data-ready={size ? "true" : "false"}
    >
      <img
        src={url}
        alt=""
        loading="lazy"
        draggable={false}
        className="absolute h-auto max-w-none"
        onLoad={(e) => setSize([e.currentTarget.naturalWidth || 1, e.currentTarget.naturalHeight || 1])}
        style={style ?? { visibility: "hidden" }}
      />
    </span>
  );
}

/** The empty frame of a person with no face picture yet. */
export function BlankFace({ className }: { className?: string }) {
  return <span className={cn("face-crop round block aspect-square w-full rounded-full bg-surface ring-1 ring-line ring-inset", className)} aria-hidden="true" />;
}
