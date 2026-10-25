import type { Loaded } from "./controller";

/**
 * The picture area of a slideshow and of the frame: the current photo fitted inside the area, the outgoing one beneath it
 * while a crossfade runs. At most two layers, so a show of days never grows the page. With `backdrop="blur"` each layer also
 * draws the photo's own preview, filled to the area, blurred and darkened, behind it (the same address, so no second download).
 */
export function SlideStage({
  slide,
  outgoing,
  fade,
  backdrop = "black",
}: {
  slide: Loaded | null;
  outgoing: Loaded | null;
  fade: boolean;
  backdrop?: "black" | "blur";
}) {
  return (
    <div className={`slide-stage${backdrop === "blur" ? " blur" : ""}`}>
      {outgoing && <Layer key={`out-${outgoing.url}`} slide={outgoing} backdrop={backdrop} cls="out" />}
      {slide && <Layer key={slide.url} slide={slide} backdrop={backdrop} cls={fade && outgoing ? "in" : ""} />}
    </div>
  );
}

function Layer({ slide, backdrop, cls }: { slide: Loaded; backdrop: "black" | "blur"; cls: string }) {
  return (
    <div className={`slide-layer ${cls}`.trim()}>
      {backdrop === "blur" && <img className="slide-backdrop" src={slide.url} alt="" aria-hidden="true" draggable={false} />}
      <img className="slide-photo" src={slide.url} alt="" draggable={false} />
    </div>
  );
}
