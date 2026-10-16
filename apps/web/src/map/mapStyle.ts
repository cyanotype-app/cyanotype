// The MapLibre style: Protomaps basemap layers over PMTiles served same-origin, labels from vendored glyphs. Nothing
// here talks to a third-party host. See docs/deploy.md ("Map") for how the archives get installed.
import { layers, namedFlavor } from "@protomaps/basemaps";
import type { StyleSpecification } from "maplibre-gl";

/** Where the assets service relays the archives' byte ranges (the bucket has no CORS). */
export const TILE_PATH = "/assets/tiles";
/** Zoom at which the optional regional extract takes over drawing detail. */
export const DETAIL_MIN_ZOOM = 8;

export interface BasemapState {
  world: boolean;
  detail: boolean;
}

/** Label-free plain background colours (match the app's surfaces) for when no basemap is installed. */
const PLAIN = { light: "#e4edf4", dark: "#0b1822" };

export function buildStyle(dark: boolean, basemap: BasemapState, origin = window.location.origin): StyleSpecification {
  const flavor = namedFlavor(dark ? "dark" : "light");
  const style: StyleSpecification = {
    version: 8,
    glyphs: `${origin}/map/fonts/{fontstack}/{range}.pbf`,
    sources: {},
    layers: [{ id: "plain-background", type: "background", paint: { "background-color": dark ? PLAIN.dark : PLAIN.light } }],
  };
  if (!basemap.world) return style;

  // POI icons need a sprite sheet this app does not ship (and the archives it installs stop at low zoom anyway).
  const base = layers("world", flavor, { lang: "en" }).filter((l) => !("layout" in l && l.layout && "icon-image" in l.layout));
  style.sources.world = { type: "vector", url: `pmtiles://${origin}${TILE_PATH}/world.pmtiles` };
  style.layers = base;
  if (basemap.detail) {
    style.sources.detail = { type: "vector", url: `pmtiles://${origin}${TILE_PATH}/detail.pmtiles` };
    for (const l of layers("detail", flavor, { lang: "en" })) {
      if (l.type === "background" || ("layout" in l && l.layout && "icon-image" in l.layout)) continue;
      style.layers.push({ ...l, id: `detail-${l.id}`, minzoom: Math.max(l.minzoom ?? 0, DETAIL_MIN_ZOOM) } as typeof l);
    }
  }
  return style;
}
