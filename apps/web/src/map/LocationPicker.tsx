// The map of the "set location" dialog: tap (or drag the pin) to choose a point. Same style and same-origin tiles as the
// map page; loaded lazily with the dialog.
import "maplibre-gl/dist/maplibre-gl.css";
import { useEffect, useMemo, useRef } from "react";
import type { Map as MapLibreMap, Marker } from "maplibre-gl";
import { loadMapLibre } from "./maplibre";
import { buildStyle, type BasemapState } from "./mapStyle";

export interface Point {
  latitude: number;
  longitude: number;
}

interface Props {
  point: Point | null;
  basemap: BasemapState;
  dark: boolean;
  onPick: (p: Point) => void;
  onUnavailable?: () => void;
}

function pin(): HTMLElement {
  const el = document.createElement("div");
  el.className = "place-pin";
  el.innerHTML =
    '<svg viewBox="0 0 24 32" width="28" height="36"><path d="M12 31C12 31 2 19.5 2 12a10 10 0 0 1 20 0c0 7.5-10 19-10 19z" fill="currentColor" stroke="#fff" stroke-width="2"/><circle cx="12" cy="12" r="3.5" fill="#fff"/></svg>';
  return el;
}

/** Zoom to show a place that was chosen from the list or typed in. */
const PICK_ZOOM = 9;

export default function LocationPicker({ point, basemap, dark, onPick, onUnavailable }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const markerRef = useRef<Marker | null>(null);
  const style = useMemo(() => buildStyle(dark, basemap), [dark, basemap]);
  const styleRef = useRef(style);
  const cb = useRef({ onPick, onUnavailable });
  const pointRef = useRef(point);
  useEffect(() => {
    cb.current = { onPick, onUnavailable };
    pointRef.current = point;
  });
  // The point the map itself reported last, so that an echo of it does not move the view.
  const own = useRef<Point | null>(null);

  useEffect(() => {
    let cancelled = false;
    let map: MapLibreMap | null = null;
    void (async () => {
      try {
        const maplibre = await loadMapLibre();
        if (cancelled || !host.current) return;
        const start = pointRef.current;
        map = new maplibre.Map({
          container: host.current,
          style: styleRef.current,
          center: start ? [start.longitude, start.latitude] : [10, 25],
          zoom: start ? PICK_ZOOM : 1.4,
          attributionControl: false,
          dragRotate: false,
          fadeDuration: 0,
        });
        map.touchZoomRotate.disableRotation();
        map.addControl(new maplibre.NavigationControl({ showCompass: false }), "top-right");
        const put = (lng: number, lat: number) => {
          own.current = { latitude: lat, longitude: lng };
          cb.current.onPick(own.current);
        };
        const marker = new maplibre.Marker({ element: pin(), anchor: "bottom", draggable: true });
        marker.on("dragend", () => {
          const l = marker.getLngLat();
          put(l.lng, l.lat);
        });
        if (start) marker.setLngLat([start.longitude, start.latitude]).addTo(map);
        markerRef.current = marker;
        map.on("click", (e) => {
          const wrapped = ((((e.lngLat.lng + 180) % 360) + 360) % 360) - 180;
          marker.setLngLat([wrapped, e.lngLat.lat]).addTo(map!);
          put(wrapped, e.lngLat.lat);
        });
        mapRef.current = map;
      } catch {
        if (!cancelled) cb.current.onUnavailable?.();
      }
    })();
    return () => {
      cancelled = true;
      map?.remove();
      mapRef.current = null;
      markerRef.current = null;
    };
  }, []);

  // A point chosen elsewhere (a place from the list, coordinates typed in): move the pin and the view there.
  useEffect(() => {
    const map = mapRef.current;
    const marker = markerRef.current;
    if (!map || !marker || !point) return;
    if (own.current && own.current.latitude === point.latitude && own.current.longitude === point.longitude) return;
    marker.setLngLat([point.longitude, point.latitude]).addTo(map);
    map.flyTo({ center: [point.longitude, point.latitude], zoom: Math.max(map.getZoom(), PICK_ZOOM), duration: 400 });
  }, [point]);

  const last = useRef(style);
  useEffect(() => {
    styleRef.current = style;
    if (last.current === style) return;
    last.current = style;
    mapRef.current?.setStyle(style);
  }, [style]);

  return <div className="location-picker" ref={host} role="application" aria-label="Map: tap to place the pin" />;
}
