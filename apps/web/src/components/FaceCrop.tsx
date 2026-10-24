import { useState } from "react";
import type { FaceBox } from "../api";
import { cropStyle } from "../peopleText";

/**
 * A face, cropped out of its photo's preview with CSS: the preview is drawn big inside a square, overflow-hidden box so only the
 * face shows. No new file is made; the boxes are fractions of the preview, so its size on the wire is read from the loaded picture.
 * Decorative (`alt=""`): whatever holds it (a link, a button) carries the name.
 */
export function FaceCrop({ url, box, round = true, className = "" }: { url: string; box: FaceBox; round?: boolean; className?: string }) {
  const [size, setSize] = useState<[number, number] | null>(null);
  const style = size ? cropStyle(box, size[0], size[1]) : null;
  return (
    <span className={`face-crop${round ? " round" : ""} ${className}`.trim()} data-ready={size ? "true" : "false"}>
      <img
        src={url}
        alt=""
        loading="lazy"
        draggable={false}
        onLoad={(e) => setSize([e.currentTarget.naturalWidth || 1, e.currentTarget.naturalHeight || 1])}
        style={style ?? { visibility: "hidden" }}
      />
    </span>
  );
}
