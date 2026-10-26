import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import type { SlideSource } from "./deck";
import { Button } from "../ui/Button";
import { enterFullscreen } from "./device";
import { PlayIcon } from "./icons";
import { Slideshow } from "./Slideshow";
import "../viewer.css";

interface Host {
  /** Start a slideshow of `source`. Call it from a click or key handler: that is what lets the browser go full screen. */
  start: (source: SlideSource) => void;
}

const SlideshowContext = createContext<Host | null>(null);

/** Holds the one slideshow that can be open and offers `start` to the pages below it. */
export function SlideshowProvider({ children }: { children: ReactNode }) {
  const [source, setSource] = useState<SlideSource | null>(null);
  const start = useCallback((s: SlideSource) => {
    void enterFullscreen();
    setSource(s);
  }, []);
  const host = useMemo(() => ({ start }), [start]);
  return (
    <SlideshowContext.Provider value={host}>
      {children}
      {source && <Slideshow source={source} onClose={() => setSource(null)} />}
    </SlideshowContext.Provider>
  );
}

/** Null outside a provider (a page rendered on its own): callers then do not offer a slideshow. */
export function useSlideshow(): Host | null {
  return useContext(SlideshowContext);
}

/** The "Slideshow" button of a page header. `source` is built when it is clicked. */
export function SlideshowButton({
  source,
  label = "Slideshow",
  disabled,
}: {
  source: () => SlideSource;
  label?: string;
  disabled?: boolean;
}) {
  const host = useSlideshow();
  if (!host) return null;
  return (
    <Button variant="ghost" className="max-md:size-10 max-md:px-0" disabled={disabled} onClick={() => host.start(source())}>
      <PlayIcon />
      <span className="max-md:sr-only">{label}</span>
    </Button>
  );
}
