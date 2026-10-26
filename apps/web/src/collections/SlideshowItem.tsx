import { Play } from "lucide-react";
import { useSlideshow } from "../slideshow/SlideshowHost";
import type { SlideSource } from "../slideshow/deck";
import { MenuItem } from "../ui/Menu";

/** "Slideshow" in a page's "⋯" menu; nothing where there is no slideshow host (a page rendered on its own). `source` is built on choosing it. */
export function SlideshowItem({ source, disabled }: { source: () => SlideSource; disabled?: boolean }) {
  const host = useSlideshow();
  if (!host) return null;
  return (
    <MenuItem disabled={disabled} onSelect={() => host.start(source())}>
      <Play />
      Slideshow
    </MenuItem>
  );
}
