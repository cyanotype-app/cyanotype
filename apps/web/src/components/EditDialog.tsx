import { lazy, Suspense, useEffect, useRef, useState, type FormEvent } from "react";
import {
  api,
  ApiError,
  type EditRequest,
  type EditResult,
  type PhotoDetails,
  type PlaceHit,
  type Target,
  type TilesInstalled,
} from "../api";
import { inputLocal, OFFSETS, offsetLabel, parsePoint, setDate, shiftSeconds, shiftText, type Shift } from "../editText";
import type { Point } from "../map/LocationPicker";
import { placeLine } from "../map/geo";

// maplibre-gl is big: the map loads when the location dialog opens.
const LocationPicker = lazy(() => import("../map/LocationPicker"));

export type EditKind = "date" | "location" | "caption";

interface Props {
  kind: EditKind;
  /** The photos to edit (explicit ids, or a view's scope: at most 1000). */
  target: Target;
  count: number;
  /** One photo's present values, to start from (absent for several photos). */
  current?: Pick<PhotoDetails, "date" | "caption" | "edits"> & { location: Point | null };
  onClose: () => void;
  onDone: (result: EditResult) => void;
}

const TITLE: Record<EditKind, string> = {
  date: "Adjust date and time",
  location: "Set location",
  caption: "Edit caption",
};
const darkQuery = () => window.matchMedia?.("(prefers-color-scheme: dark)");

/**
 * Edit the date and time (with its zone; for several photos, move them all by an offset), the location (pick on the map,
 * pick one of your places, or type coordinates) or the caption of one or more photos. Nothing is written to the original:
 * the edits are kept beside it and "Revert to original" puts the file's own values back.
 */
export function EditDialog({ kind, target, count, current, onClose, onDone }: Props) {
  const many = count > 1;
  const formRef = useRef<HTMLFormElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // After a partial failure only the photos that failed are sent again (a shift would move the others twice).
  const [retry, setRetry] = useState<string[] | null>(null);

  // date
  const [local, setLocal] = useState(current ? inputLocal(current.date.local) : "");
  const [zone, setZone] = useState(current?.date.offset ?? "");
  const [shift, setShift] = useState<Shift>({ later: true, days: 0, hours: 0, minutes: 0 });
  // location
  const [point, setPoint] = useState<Point | null>(current?.location ?? null);
  const [coords, setCoords] = useState(current?.location ? `${current.location.latitude}, ${current.location.longitude}` : "");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<PlaceHit[] | null>(null);
  const [tiles, setTiles] = useState<TilesInstalled | null>(null);
  const [dark, setDark] = useState(() => darkQuery()?.matches ?? false);
  const [mapFailed, setMapFailed] = useState(false);
  // caption
  const [caption, setCaption] = useState(current?.caption ?? "");

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    formRef.current?.querySelector<HTMLElement>("input, textarea, select")?.focus();
    return () => prev?.focus?.();
  }, []);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (!busy) onClose();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onClose, busy]);

  useEffect(() => {
    if (kind !== "location") return;
    let live = true;
    api
      .tiles()
      .then((t) => live && setTiles(t))
      .catch(() => live && setTiles({ world: false, detail: false }));
    const q = darkQuery();
    const on = () => setDark(q?.matches ?? false);
    q?.addEventListener("change", on);
    return () => {
      live = false;
      q?.removeEventListener("change", on);
    };
  }, [kind]);

  // Your places, as you type.
  useEffect(() => {
    if (kind !== "location") return;
    const ctl = new AbortController();
    const t = setTimeout(
      () =>
        api
          .places(query.trim(), ctl.signal)
          .then((h) => !ctl.signal.aborted && setHits(h))
          .catch(() => !ctl.signal.aborted && setHits([])),
      query ? 200 : 0,
    );
    return () => {
      clearTimeout(t);
      ctl.abort();
    };
  }, [kind, query]);

  const choose = (p: Point) => {
    setPoint(p);
    setCoords(`${p.latitude.toFixed(5)}, ${p.longitude.toFixed(5)}`);
  };

  async function send(edit: EditRequest) {
    setBusy(true);
    setError(null);
    try {
      const r = await api.editAssets(retry ?? target, edit);
      if (r.failed.length > 0) {
        setRetry(r.failed);
        setError(`${r.failed.length} of ${count} could not be saved. Try again to save the rest.`);
        return;
      }
      onDone(r);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Could not save. Try again.");
    } finally {
      setBusy(false);
    }
  }

  const secs = shiftSeconds(shift);
  const valid =
    kind === "date" ? (many ? secs !== 0 : local.length >= 16) : kind === "location" ? point !== null : true;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!valid || busy) return;
    if (kind === "date") void send({ date: many ? { shift_seconds: secs } : setDate(local, zone) });
    else if (kind === "location") void send({ location: { latitude: point!.latitude, longitude: point!.longitude } });
    else void send({ caption: caption.trim() ? { text: caption } : { revert: true } });
  };
  const edited =
    kind === "date" ? current?.edits?.date_edited : kind === "location" ? current?.edits?.location_edited : !!current?.caption;
  const original =
    kind === "date" && current?.edits?.original_date
      ? `${current.edits.original_date.local.replace("T", " ")}${current.edits.original_date.offset ? ` ${offsetLabel(current.edits.original_date.offset)}` : ""}`
      : kind === "location" && current?.edits?.original_location
        ? `${current.edits.original_location.latitude.toFixed(4)}, ${current.edits.original_location.longitude.toFixed(4)}`
        : null;
  const label = `${TITLE[kind]}${many ? `, ${count} photos` : ""}`;

  return (
    <div className="modal-backdrop">
      <form className="modal card edit-dialog" role="dialog" aria-modal="true" aria-label={label} ref={formRef} onSubmit={submit}>
        <h2>{label}</h2>

        {kind === "date" && !many && (
          <>
            <div className="edit-field">
              <label htmlFor="edit-local">Date and time</label>
              <input
                id="edit-local"
                className="text-input"
                type="datetime-local"
                step="1"
                value={local}
                onChange={(e) => setLocal(e.target.value)}
                required
              />
            </div>
            <div className="edit-field">
              <label htmlFor="edit-zone">Time zone</label>
              <select id="edit-zone" className="text-input" value={zone} onChange={(e) => setZone(e.target.value)}>
                <option value="">Unknown</option>
                {OFFSETS.map((o) => (
                  <option key={o} value={o}>
                    {offsetLabel(o)}
                  </option>
                ))}
              </select>
            </div>
          </>
        )}
        {kind === "date" && many && (
          <>
            <p className="muted small">Move the date and time of all {count} photos by the same amount, each keeping its own time zone.</p>
            <div className="edit-shift">
              <label className="sr-only" htmlFor="edit-dir">
                Direction
              </label>
              <select id="edit-dir" className="text-input" value={shift.later ? "later" : "earlier"} onChange={(e) => setShift({ ...shift, later: e.target.value === "later" })}>
                <option value="later">Later</option>
                <option value="earlier">Earlier</option>
              </select>
              {(["days", "hours", "minutes"] as const).map((u) => (
                <div key={u} className="edit-field">
                  <label htmlFor={`edit-${u}`}>{u[0]!.toUpperCase() + u.slice(1)}</label>
                  <input
                    id={`edit-${u}`}
                    className="text-input"
                    type="number"
                    min={0}
                    inputMode="numeric"
                    value={shift[u]}
                    onChange={(e) => setShift({ ...shift, [u]: e.target.valueAsNumber })}
                  />
                </div>
              ))}
            </div>
            <p className="small" role="status">
              {shiftText(shift)}
            </p>
          </>
        )}

        {kind === "location" && (
          <>
            <div className="edit-field">
              <label htmlFor="edit-place">Search your places</label>
              <input id="edit-place" className="text-input" type="search" placeholder="Lisbon" value={query} onChange={(e) => setQuery(e.target.value)} autoComplete="off" />
            </div>
            {hits && hits.length > 0 && (
              <ul className="edit-places" aria-label="Your places">
                {hits.slice(0, 6).map((h) => (
                  <li key={`${h.city}|${h.region}|${h.country}`}>
                    <button type="button" className="btn btn-quiet" onClick={() => choose({ latitude: h.latitude, longitude: h.longitude })}>
                      {placeLine(h)} <span className="muted">{h.count.toLocaleString("en-US")}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {hits && hits.length === 0 && query && <p className="muted small">None of your photos is in a place like that.</p>}
            {!mapFailed && tiles && (
              <Suspense fallback={<div className="location-picker" />}>
                <LocationPicker point={point} basemap={tiles} dark={dark} onPick={choose} onUnavailable={() => setMapFailed(true)} />
              </Suspense>
            )}
            {mapFailed && <p className="muted small">The map cannot be drawn in this browser: type the coordinates instead.</p>}
            <div className="edit-field">
              <label htmlFor="edit-coords">Coordinates</label>
              <input
                id="edit-coords"
                className="text-input"
                placeholder="38.7223, -9.1393"
                value={coords}
                onChange={(e) => {
                  setCoords(e.target.value);
                  const p = parsePoint(e.target.value);
                  if (p) setPoint(p);
                }}
                aria-invalid={coords !== "" && parsePoint(coords) === null}
              />
            </div>
          </>
        )}

        {kind === "caption" && (
          <div className="edit-field">
            <label htmlFor="edit-caption">{many ? `Caption for all ${count} photos` : "Caption"}</label>
            <textarea id="edit-caption" className="text-input" rows={4} maxLength={2000} value={caption} onChange={(e) => setCaption(e.target.value)} />
            <p className="muted small">Searching finds photos by the words in their captions.</p>
          </div>
        )}

        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        {original && <p className="muted small">The file says {original}.</p>}
        <div className="modal-actions">
          {(edited || many) && kind !== "caption" && (
            <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => void send({ [kind]: { revert: true } } as EditRequest)}>
              Revert to original
            </button>
          )}
          {kind === "caption" && (many || !!current?.caption) && (
            <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => void send({ caption: { revert: true } })}>
              Remove caption
            </button>
          )}
          <button type="button" className="btn btn-quiet" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || !valid}>
            Save
          </button>
        </div>
      </form>
    </div>
  );
}
