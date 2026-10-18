// The small, non-interactive map of the photo details panel: the basemap (same style and same-origin tiles as the map
// page) centred on the photo with a pin. Loaded lazily with the panel's map; the link around it opens /map there.
import "maplibre-gl/dist/maplibre-gl.css";
import { useEffect, useMemo, useRef } from "react";
import type { Map as MapLibreMap } from "maplibre-gl";
import { loadMapLibre } from "./maplibre";
import { buildStyle, type BasemapState } from "./mapStyle";

interface Props {
  lat: number;
  lon: number;
  basemap: BasemapState;
  dark: boolean;
  /** The map cannot start (typically no WebGL): the panel then shows the coordinates only. */
  onUnavailable?: () => void;
}

/** Street level is too close for a thumbnail map and the world too far; a city and its surroundings read well. */
export const PLACE_MAP_ZOOM = 10;

function pinElement(): HTMLElement {
  const el = document.createElement("div");
  el.className = "place-pin";
  el.setAttribute("aria-hidden", "true");
  el.innerHTML =
    '<svg viewBox="0 0 24 32" width="24" height="32"><path d="M12 31C12 31 2 19.5 2 12a10 10 0 0 1 20 0c0 7.5-10 19-10 19z" fill="currentColor" stroke="#fff" stroke-width="2"/><circle cx="12" cy="12" r="3.5" fill="#fff"/></svg>';
  return el;
}

export default function PlaceMap({ lat, lon, basemap, dark, onUnavailable }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const style = useMemo(() => buildStyle(dark, basemap), [dark, basemap]);
  const styleRef = useRef(style);
  const unavailable = useRef(onUnavailable);
  useEffect(() => {
    unavailable.current = onUnavailable;
  });

  useEffect(() => {
    let cancelled = false;
    let map: MapLibreMap | null = null;
    void (async () => {
      try {
        const maplibre = await loadMapLibre();
        if (cancelled || !host.current) return;
        map = new maplibre.Map({
          container: host.current,
          style: styleRef.current,
          center: [lon, lat],
          zoom: PLACE_MAP_ZOOM,
          interactive: false, // a picture of where, not a second map to fight over scrolling with
          attributionControl: false,
          fadeDuration: 0,
        });
        new maplibre.Marker({ element: pinElement(), anchor: "bottom" }).setLngLat([lon, lat]).addTo(map);
        mapRef.current = map;
      } catch {
        if (!cancelled) unavailable.current?.();
      }
    })();
    return () => {
      cancelled = true;
      map?.remove();
      mapRef.current = null;
    };
  }, [lat, lon]);

  // Light/dark or a newly installed basemap.
  const lastStyle = useRef(style);
  useEffect(() => {
    styleRef.current = style;
    if (lastStyle.current === style) return;
    lastStyle.current = style;
    mapRef.current?.setStyle(style);
  }, [style]);

  return <div className="place-map" ref={host} aria-hidden="true" />;
}
