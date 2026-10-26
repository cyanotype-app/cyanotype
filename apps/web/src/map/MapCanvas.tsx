// The MapLibre map: basemap, photo markers and the viewport it reports. Loaded lazily (maplibre-gl is ~800 kB) by the
// map page; everything MapLibre-specific lives here so the page can be tested against a stand-in for this component.
import "maplibre-gl/dist/maplibre-gl.css";
import { useEffect, useMemo, useRef } from "react";
import type { Map as MapLibreMap, Marker } from "maplibre-gl";
import type { MapCluster } from "../api";
import { placeholderUrl } from "../thumbhash";
import { bubbleSize, clusterTarget, shortCount, type Bounds } from "./geo";
import { loadMapLibre } from "./maplibre";
import { buildStyle, type BasemapState } from "./mapStyle";

export interface MapViewport {
  bounds: Bounds;
  /** Fractional MapLibre zoom (512 px tiles). */
  zoom: number;
}

interface Props {
  clusters: readonly MapCluster[];
  basemap: BasemapState;
  dark: boolean;
  /** Fit the view to this box when it changes (the page passes the extent of the library once). */
  fitTo: Bounds | null;
  /** The view settled (after a pan or zoom) and once the map is ready. */
  onViewport: (v: MapViewport) => void;
  onOpenPhoto: (id: string) => void;
  /** The map cannot start (typically no WebGL). */
  onUnavailable: (reason: string) => void;
  /** Pixels of the map that bars and panels cover; the view is fitted to what is left. */
  inset?: { top: number; right: number; bottom: number; left: number };
}

const MAX_ZOOM = 19;

function viewport(map: MapLibreMap): MapViewport {
  const b = map.getBounds();
  return { bounds: { west: b.getWest(), south: b.getSouth(), east: b.getEast(), north: b.getNorth() }, zoom: map.getZoom() };
}

const sig = (c: MapCluster) => `${c.id}|${c.count}|${c.lat.toFixed(5)}|${c.lon.toFixed(5)}|${c.thumb_url ?? ""}`;

function markerElement(c: MapCluster, onClick: () => void): HTMLElement {
  const el = document.createElement("button");
  el.type = "button";
  if (c.count > 1) {
    const size = bubbleSize(c.count);
    el.dataset.marker = "cluster";
    el.className =
      "flex cursor-pointer items-center justify-center rounded-full border-2 border-white bg-accent p-0 text-[0.8125rem] leading-none font-bold " +
      "text-on-accent shadow-[0_1px_4px_rgb(0_19_31/0.45)] focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-focus";
    el.style.width = el.style.height = `${size}px`;
    el.textContent = shortCount(c.count);
    el.setAttribute("aria-label", `${c.count} photos, zoom in`);
  } else {
    el.dataset.marker = "photo";
    el.className =
      "flex size-[3.25rem] cursor-pointer items-center justify-center overflow-hidden rounded-lg border-2 border-white bg-accent/40 bg-cover bg-center p-0 " +
      "shadow-[0_1px_4px_rgb(0_19_31/0.45)] focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-focus";
    el.setAttribute("aria-label", "Open photo");
    const bg = placeholderUrl(c.thumbhash);
    if (bg) el.style.backgroundImage = `url(${bg})`;
    if (c.thumb_url) {
      const img = document.createElement("img");
      img.src = c.thumb_url;
      img.alt = "";
      img.draggable = false;
      img.className = "pointer-events-none block size-full object-cover";
      img.loading = "lazy";
      el.appendChild(img);
    }
  }
  el.addEventListener("click", (e) => {
    e.stopPropagation();
    onClick();
  });
  return el;
}

const NO_INSET = { top: 0, right: 0, bottom: 0, left: 0 };

export default function MapCanvas({ clusters, basemap, dark, fitTo, onViewport, onOpenPhoto, onUnavailable, inset = NO_INSET }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const lib = useRef<typeof import("maplibre-gl") | null>(null);
  const markers = useRef(new Map<string, { marker: Marker; el: HTMLElement }>());
  // Latest callbacks, so the map is created once and never rebuilt for a new closure.
  const cb = useRef({ onViewport, onOpenPhoto, onUnavailable });
  useEffect(() => {
    cb.current = { onViewport, onOpenPhoto, onUnavailable };
  });
  const styleKey = `${dark}|${basemap.world}|${basemap.detail}`;
  const style = useMemo(() => buildStyle(dark, basemap), [dark, basemap]);
  const styleRef = useRef(style);
  const clustersRef = useRef(clusters);
  const fitRef = useRef(fitTo);
  const insetRef = useRef(inset);

  // Create the map once.
  useEffect(() => {
    let cancelled = false;
    let map: MapLibreMap | null = null;
    const els = markers.current;
    void (async () => {
      try {
        const maplibre = await loadMapLibre();
        if (cancelled || !host.current) return;
        lib.current = maplibre;
        map = new maplibre.Map({
          container: host.current,
          style: styleRef.current,
          center: [10, 25],
          zoom: 1.4,
          maxZoom: MAX_ZOOM,
          attributionControl: false,
          dragRotate: false,
          pitchWithRotate: false,
          fadeDuration: 0,
        });
        map.touchZoomRotate.disableRotation();
        map.addControl(new maplibre.NavigationControl({ showCompass: false }), "top-right");
        mapRef.current = map;
        map.on("moveend", () => cb.current.onViewport(viewport(map!)));
        // Not on "load": that waits for the style and the first tiles, and a slow or missing basemap must not
        // keep the markers and the photo list from appearing.
        if (fitRef.current) fit(map, fitRef.current);
        cb.current.onViewport(viewport(map));
        draw();
      } catch (e) {
        if (!cancelled) cb.current.onUnavailable(e instanceof Error ? e.message : "The map could not start.");
      }
    })();
    return () => {
      cancelled = true;
      for (const { marker } of els.values()) marker.remove();
      els.clear();
      map?.remove();
      mapRef.current = null;
    };
  }, []);

  function fit(map: MapLibreMap, b: Bounds) {
    const degenerate = Math.abs(b.east - b.west) < 1e-4 && Math.abs(b.north - b.south) < 1e-4;
    if (degenerate) map.jumpTo({ center: [b.west, b.south], zoom: 12, padding: insetRef.current });
    else {
      const pad = insetRef.current;
      map.fitBounds([[b.west, b.south], [b.east, b.north]], {
        padding: { top: 48 + pad.top, right: 48 + pad.right, bottom: 48 + pad.bottom, left: 48 + pad.left },
        maxZoom: 14,
        duration: 0,
      });
    }
  }

  // Markers: add what is new, drop what is gone, leave the rest untouched (no flicker while panning).
  function draw() {
    const map = mapRef.current;
    const maplibre = lib.current;
    if (!map || !maplibre) return;
    const wanted = new Map(clustersRef.current.map((c) => [sig(c), c]));
    for (const [key, m] of markers.current) {
      if (!wanted.has(key)) {
        m.marker.remove();
        markers.current.delete(key);
      }
    }
    for (const [key, c] of wanted) {
      if (markers.current.has(key)) continue;
      const el = markerElement(c, () => {
        if (c.count === 1) return cb.current.onOpenPhoto(c.id);
        const t = clusterTarget(c, map.getZoom(), MAX_ZOOM);
        if (t.kind === "bounds") map.fitBounds(t.bounds, { padding: 72, maxZoom: MAX_ZOOM, duration: 450 });
        else map.easeTo({ center: t.center, zoom: t.zoom, duration: 450 });
      });
      const marker = new maplibre.Marker({ element: el, anchor: "center" }).setLngLat([c.lon, c.lat]).addTo(map);
      markers.current.set(key, { marker, el });
    }
  }

  useEffect(() => {
    clustersRef.current = clusters;
    draw();
  }, [clusters]);

  useEffect(() => {
    insetRef.current = inset;
  });

  useEffect(() => {
    fitRef.current = fitTo;
    const map = mapRef.current;
    if (map && fitTo) fit(map, fitTo);
  }, [fitTo]);

  // Light/dark or a newly installed basemap: swap the style; markers are DOM elements and stay.
  const lastStyle = useRef(styleKey);
  useEffect(() => {
    styleRef.current = style;
    if (lastStyle.current === styleKey) return;
    lastStyle.current = styleKey;
    mapRef.current?.setStyle(style);
  }, [style, styleKey]);

  return <div className="map-canvas size-full" ref={host} role="application" aria-label="Photo map" />;
}
