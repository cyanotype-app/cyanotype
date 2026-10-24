import { useEffect, useId, useRef, useState } from "react";
import { api, type Album, type Facets, type Person, type SearchParams, type Tag } from "../api";
import { MAX_FILTER_PEOPLE, NO_NAMED_PEOPLE, PEOPLE_FILTER_NOTE, peopleIdsOf } from "../peopleText";
import { ratingFilterLabel } from "../ratingText";
import { tagListOf } from "../tagText";
import { PeoplePicker } from "./PeoplePicker";
import { TagInput } from "./TagInput";
import { TYPE_LABEL, yearRange } from "../searchText";

type Filters = Omit<SearchParams, "q">;

/** Autocomplete for the tag filter: the library's tags containing what is typed. */
const defaultTagLoad = (typed: string) => api.listTags({ q: typed || undefined, sort: "count", limit: 12 }).then((r) => r.items);

interface Props {
  /** The filters in force now. */
  value: Filters;
  /** Apply the edited filters. */
  onApply: (next: Filters) => void;
  onClose: () => void;
  /** Where the options come from (the tests pass fakes). */
  load?: () => Promise<{ facets: Facets; albums: Album[] }>;
  /** Where the tag names come from (the tests pass fakes). */
  tagLoad?: (typed: string) => Promise<Tag[]>;
  /** The people the filter can choose (named, not hidden). Undefined while People is off: the filter is not offered then. */
  people?: readonly Person[];
}

const defaultLoad = async () => {
  // A smart album has no members to filter by (its rules are the filters).
  const [facets, albums] = await Promise.all([api.searchFacets(), api.listAlbums().then((r) => r.items.filter((a) => !a.smart))]);
  return { facets, albums };
};

const n = (v: number) => v.toLocaleString("en-US");
const yearOf = (f: Filters) => (f.from && f.to && f.from === yearRange(f.from.slice(0, 4)).from && f.to === yearRange(f.from.slice(0, 4)).to ? f.from.slice(0, 4) : "");

/**
 * Filters for the search: date range (or a whole year), favorites, camera, place, file type, location and album. A
 * bottom sheet on a phone, a dialog on a wide screen; edits apply together with "Show photos", and the choices come from
 * what the library has (`GET /assets/search/facets`) with counts.
 */
export function FilterSheet({ value, onApply, onClose, load = defaultLoad, tagLoad = defaultTagLoad, people }: Props) {
  const [draft, setDraft] = useState<Filters>(value);
  const [options, setOptions] = useState<{ facets: Facets; albums: Album[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const id = useId();

  useEffect(() => {
    let live = true;
    load().then(
      (o) => live && setOptions(o),
      (e: unknown) => live && setError(e instanceof Error ? e.message : "Could not load the filter options."),
    );
    return () => {
      live = false;
    };
  }, [load]);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("select, input, button")?.focus();
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      } else if (e.key === "Tab" && ref.current) {
        const f = ref.current.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), select:not(:disabled)");
        const first = f[0];
        const last = f[f.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last?.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first?.focus();
        }
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      document.body.style.overflow = overflow;
      prev?.focus?.();
    };
  }, [onClose]);

  const set = (patch: Partial<Filters>) =>
    setDraft((d) => {
      const next = { ...d, ...patch };
      for (const k of Object.keys(next) as (keyof Filters)[]) if (next[k] === undefined || next[k] === "") delete next[k];
      return next;
    });

  const { facets, albums } = options ?? { facets: null, albums: [] as Album[] };
  // A value the URL carries but the library does not list (an old link, a renamed place) stays selectable.
  const withCurrent = <T extends { value: string }>(list: T[], current: string | undefined, make: (v: string) => T) =>
    current && !list.some((x) => x.value.toLowerCase() === current.toLowerCase()) ? [make(current), ...list] : list;
  const cameras = withCurrent(facets?.cameras ?? [], draft.camera, (v) => ({ value: v, count: 0 }));
  const lenses = withCurrent(facets?.lenses ?? [], draft.lens, (v) => ({ value: v, count: 0 }));
  const places = withCurrent(facets?.places ?? [], draft.place, (v) => ({ value: v, count: 0, kind: "city" as const }));
  const year = yearOf(draft);
  const years = withCurrent(facets?.years ?? [], year || undefined, (v) => ({ value: v, count: 0 }));
  const count = Object.keys(draft).length;
  const label = (what: string, v: { value: string; count: number }) => (v.count > 0 ? `${v.value} (${n(v.count)})` : v.value) || what;

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet card" role="dialog" aria-modal="true" aria-labelledby={`${id}-title`} ref={ref}>
        <div className="sheet-grip" aria-hidden="true" />
        <h2 id={`${id}-title`}>Filters</h2>
        {error && (
          <p role="alert" className="form-error small">
            {error}
          </p>
        )}
        <div className="sheet-body">
          <div className="sheet-field">
            <label htmlFor={`${id}-year`}>Year</label>
            <select
              id={`${id}-year`}
              className="text-input"
              value={year}
              onChange={(e) => set(e.target.value ? yearRange(e.target.value) : { from: undefined, to: undefined })}
            >
              <option value="">Any</option>
              {years.map((y) => (
                <option key={y.value} value={y.value}>
                  {label("", y)}
                </option>
              ))}
            </select>
          </div>
          <div className="sheet-pair">
            <div className="sheet-field">
              <label htmlFor={`${id}-from`}>From</label>
              <input
                id={`${id}-from`}
                className="text-input"
                type="date"
                value={draft.from ?? ""}
                max={draft.to}
                onChange={(e) => set({ from: e.target.value })}
              />
            </div>
            <div className="sheet-field">
              <label htmlFor={`${id}-to`}>To</label>
              <input
                id={`${id}-to`}
                className="text-input"
                type="date"
                value={draft.to ?? ""}
                min={draft.from}
                onChange={(e) => set({ to: e.target.value })}
              />
            </div>
          </div>
          <label className="sheet-check">
            <input type="checkbox" checked={draft.favorite === true} onChange={(e) => set({ favorite: e.target.checked || undefined })} />
            <span>Favorites only{facets ? ` (${n(facets.favorites)})` : ""}</span>
          </label>
          <div className="sheet-field">
            <TagInput
              label="Tags"
              placeholder="Any tag"
              existingOnly
              value={tagListOf(draft.tags)}
              onChange={(names) => set({ tags: names.length ? names.join(",") : undefined, tag_mode: names.length > 1 ? draft.tag_mode : undefined })}
              load={tagLoad}
            />
            {tagListOf(draft.tags).length > 1 && (
              <div className="sheet-segments" role="group" aria-label="Match">
                {(["all", "any"] as const).map((m) => (
                  <label key={m} className="sheet-segment">
                    <input type="radio" name={`${id}-tagmode`} checked={(draft.tag_mode ?? "all") === m} onChange={() => set({ tag_mode: m === "any" ? "any" : undefined })} />
                    <span>{m === "all" ? "All of them" : "Any of them"}</span>
                  </label>
                ))}
              </div>
            )}
          </div>
          {people !== undefined &&
            (people.length === 0 && !draft.people ? (
              <div className="sheet-field">
                <span className="sheet-label">People</span>
                <p className="muted small">{NO_NAMED_PEOPLE}</p>
              </div>
            ) : (
              <div className="sheet-field">
                <PeoplePicker
                  label="People"
                  people={people}
                  value={peopleIdsOf(draft.people)}
                  max={MAX_FILTER_PEOPLE}
                  onChange={(ids) => set({ people: ids.length ? ids.join(",") : undefined })}
                />
                <p className="muted small">{PEOPLE_FILTER_NOTE}</p>
              </div>
            ))}
          <div className="sheet-field">
            <label htmlFor={`${id}-rating`}>Rating</label>
            <select
              id={`${id}-rating`}
              className="text-input"
              value={draft.rating ?? ""}
              onChange={(e) => set({ rating: e.target.value ? Number(e.target.value) : undefined })}
            >
              <option value="">Any</option>
              {[5, 4, 3, 2, 1].map((r) => {
                const c = facets?.ratings?.find((f) => f.value === String(r))?.count;
                return (
                  <option key={r} value={r}>
                    {ratingFilterLabel(r)}
                    {c !== undefined ? ` (${n(c)})` : ""}
                  </option>
                );
              })}
            </select>
          </div>
          <label className="sheet-check">
            <input
              type="checkbox"
              checked={draft.include_archived !== false}
              onChange={(e) => set({ include_archived: e.target.checked ? undefined : false })}
            />
            <span>Include archived</span>
          </label>
          <div className="sheet-field">
            <label htmlFor={`${id}-camera`}>Camera</label>
            <select id={`${id}-camera`} className="text-input" value={draft.camera ?? ""} onChange={(e) => set({ camera: e.target.value })}>
              <option value="">Any</option>
              {cameras.map((c) => (
                <option key={c.value} value={c.value}>
                  {label("", c)}
                </option>
              ))}
            </select>
          </div>
          {lenses.length > 0 && (
            <div className="sheet-field">
              <label htmlFor={`${id}-lens`}>Lens</label>
              <select id={`${id}-lens`} className="text-input" value={draft.lens ?? ""} onChange={(e) => set({ lens: e.target.value })}>
                <option value="">Any</option>
                {lenses.map((l) => (
                  <option key={l.value} value={l.value}>
                    {label("", l)}
                  </option>
                ))}
              </select>
            </div>
          )}
          <div className="sheet-field">
            <label htmlFor={`${id}-place`}>Place</label>
            <select id={`${id}-place`} className="text-input" value={draft.place ?? ""} onChange={(e) => set({ place: e.target.value })}>
              <option value="">Anywhere</option>
              {(["country", "region", "city"] as const).map((kind) => {
                const group = places.filter((p) => p.kind === kind);
                return group.length === 0 ? null : (
                  <optgroup key={kind} label={kind === "country" ? "Countries" : kind === "region" ? "Regions" : "Cities"}>
                    {group.map((p) => (
                      <option key={`${kind}:${p.value}`} value={p.value}>
                        {label("", p)}
                      </option>
                    ))}
                  </optgroup>
                );
              })}
            </select>
          </div>
          <fieldset className="sheet-field sheet-types">
            <legend>File type</legend>
            <div className="sheet-segments">
              {([undefined, "jpeg", "heic", "raw", "other"] as const).map((t) => (
                <label key={t ?? "any"} className="sheet-segment">
                  <input type="radio" name={`${id}-type`} checked={draft.type === t} onChange={() => set({ type: t })} />
                  <span>{t ? TYPE_LABEL[t] : "Any"}</span>
                </label>
              ))}
            </div>
          </fieldset>
          <div className="sheet-field">
            <label htmlFor={`${id}-loc`}>Location</label>
            <select
              id={`${id}-loc`}
              className="text-input"
              value={draft.has_location === undefined ? "" : String(draft.has_location)}
              onChange={(e) => set({ has_location: e.target.value === "" ? undefined : e.target.value === "true" })}
            >
              <option value="">Any</option>
              <option value="true">Has a location{facets ? ` (${n(facets.with_location)})` : ""}</option>
              <option value="false">No location{facets ? ` (${n(facets.total - facets.with_location)})` : ""}</option>
            </select>
          </div>
          <div className="sheet-field">
            <label htmlFor={`${id}-text`}>Text in the photo</label>
            <select
              id={`${id}-text`}
              className="text-input"
              value={draft.has_text === undefined ? "" : String(draft.has_text)}
              onChange={(e) => set({ has_text: e.target.value === "" ? undefined : e.target.value === "true" })}
            >
              <option value="">Any</option>
              <option value="true">Has text (signs, receipts, screenshots){facets?.with_text !== undefined ? ` (${n(facets.with_text)})` : ""}</option>
              <option value="false">No text</option>
            </select>
          </div>
          <div className="sheet-field">
            <label htmlFor={`${id}-album`}>Album</label>
            <select id={`${id}-album`} className="text-input" value={draft.album ?? ""} onChange={(e) => set({ album: e.target.value })}>
              <option value="">Any</option>
              {draft.album && !albums.some((a) => a.id === draft.album) && <option value={draft.album}>(selected album)</option>}
              {albums.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.title} ({n(a.photo_count)})
                </option>
              ))}
            </select>
          </div>
        </div>
        <div className="sheet-actions">
          <button type="button" className="btn btn-quiet" disabled={count === 0} onClick={() => setDraft({})}>
            Clear all
          </button>
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" onClick={() => onApply(draft)}>
            Show photos
          </button>
        </div>
      </div>
    </div>
  );
}
