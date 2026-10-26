import { useEffect, useRef, useState } from "react";
import { placeholderUrl } from "../thumbhash";
import { cn } from "../ui/cn";

/**
 * A photo filling its box: the ThumbHash blur while the picture loads, then the picture fading in over it. A box with no
 * photo (an empty album) is a plain well. Decorative: whatever holds it carries the name.
 */
export function Cover({
  src,
  thumbhash,
  className,
  imgClassName,
}: {
  src: string | null | undefined;
  thumbhash?: string | null;
  className?: string;
  imgClassName?: string;
}) {
  const bg = placeholderUrl(thumbhash ?? null);
  const img = useRef<HTMLImageElement>(null);
  const [shown, setShown] = useState(false);
  // A cached picture is complete before React attaches onLoad.
  useEffect(() => {
    if (img.current?.complete && img.current.naturalWidth > 0) setShown(true);
  }, [src]);
  return (
    <span
      className={cn("relative block overflow-hidden bg-surface bg-cover bg-center", className)}
      style={bg ? { backgroundImage: `url(${bg})` } : undefined}
    >
      {src && (
        <img
          ref={img}
          src={src}
          alt=""
          loading="lazy"
          decoding="async"
          draggable={false}
          onLoad={() => setShown(true)}
          className={cn(
            "absolute inset-0 size-full object-cover transition-opacity duration-300 ease-soft",
            shown ? "opacity-100" : "opacity-0",
            imgClassName,
          )}
        />
      )}
    </span>
  );
}
