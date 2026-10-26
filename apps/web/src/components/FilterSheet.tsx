import { useEffect, useId, useState, type ReactNode } from "react";
import { api, type Album, type Facets, type Person, type SearchParams, type Tag } from "../api";
import { MAX_FILTER_PEOPLE, NO_NAMED_PEOPLE, PEOPLE_FILTER_NOTE, peopleIdsOf } from "../peopleText";
import { ratingFilterLabel } from "../ratingText";
import { tagListOf } from "../tagText";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { Switch } from "../ui/Controls";
import { Sheet } from "../ui/Sheet";
import { PeoplePicker } from "./PeoplePicker";
import { TagInput } from "./TagInput";
import { TYPE_LABEL, yearRange } from "../searchText";

type Filters = Omit<SearchParams, "q">;

/** Autocomplete for the tag filter: the library's tags containing what is typed. */
const defaultTagLoad = (typed: string) => api.listTags({ q: typed || undefined, sort: "count", limit: 12 }).then((r) => r.items);

interface Props {
  /** Shown or hidden; the sheet stays mounted so it can animate out. */
  open?: boolean;
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

const fieldCls = "h-11 w-full min-w-0 rounded-xl bg-surface px-3 text-base text-fg outline-none focus-visible:ring-2 focus-visible:ring-accent";

/**
 * Filters for the search: date range (or a whole year), favorites, tags, people, rating, camera, place, file type,
 * location, text and album. A bottom sheet on a phone, a dialog on a wide screen; edits apply together with "Show
 * photos", and the choices come from what the library has (`GET /assets/search/facets`) with counts.
 */
export function FilterSheet({ open = true, value, onApply, onClose, load = defaultLoad, tagLoad = defaultTagLoad, people }: Props) {
  const [draft, setDraft] = useState<Filters>(value);
  const [options, setOptions] = useState<{ facets: Facets; albums: Album[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const id = useId();

  // Each time the sheet opens it starts from the filters in force, not from the last abandoned edit.
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setDraft(value);
  }

  // The library's options are read when the sheet is first opened (and again if that failed).
  const needOptions = open && options === null;
  useEffect(() => {
    if (!needOptions) return;
    let live = true;
    load().then(
      (o) => live && (setOptions(o), setError(null)),
      (e: unknown) => live && setError(e instanceof Error ? e.message : "Could not load the filter options."),
    );
    return () => {
      live = false;
    };
  }, [needOptions, load]);

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
  const label = (v: { value: string; count: number }) => (v.count > 0 ? `${v.value} (${n(v.count)})` : v.value);

  return (
    <Sheet
      open={open}
      onOpenChange={(o) => !o && onClose()}
      title="Filters"
      footer={
        <div className="flex w-full items-center gap-2">
          <Button variant="quiet" disabled={count === 0} onClick={() => setDraft({})}>
            Clear all
          </Button>
          <div className="flex-1" />
          <Button variant="secondary" className="hidden md:inline-flex" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" className="flex-1 md:flex-none" onClick={() => onApply(draft)}>
            Show photos
          </Button>
        </div>
      }
    >
      {error && (
        <p role="alert" className="mb-3 text-sm text-danger">
          {error}
        </p>
      )}
      <div className="flex flex-col gap-6 pb-2">
        <Group title="When">
          <Field label="Year" htmlFor={`${id}-year`}>
            <select
              id={`${id}-year`}
              className={fieldCls}
              value={year}
              onChange={(e) => set(e.target.value ? yearRange(e.target.value) : { from: undefined, to: undefined })}
            >
              <option value="">Any</option>
              {years.map((y) => (
                <option key={y.value} value={y.value}>
                  {label(y)}
                </option>
              ))}
            </select>
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="From" htmlFor={`${id}-from`}>
              <input
                id={`${id}-from`}
                className={fieldCls}
                type="date"
                value={draft.from ?? ""}
                max={draft.to}
                onChange={(e) => set({ from: e.target.value })}
              />
            </Field>
            <Field label="To" htmlFor={`${id}-to`}>
              <input
                id={`${id}-to`}
                className={fieldCls}
                type="date"
                value={draft.to ?? ""}
                min={draft.from}
                onChange={(e) => set({ to: e.target.value })}
              />
            </Field>
          </div>
        </Group>

        <Group title="Photos">
          <Switch
            checked={draft.favorite === true}
            onChange={(on) => set({ favorite: on || undefined })}
            label={`Favorites only${facets ? ` (${n(facets.favorites)})` : ""}`}
          />
          <div className="flex flex-col gap-2">
            <TagInput
              label="Tags"
              placeholder="Any tag"
              existingOnly
              value={tagListOf(draft.tags)}
              onChange={(names) => set({ tags: names.length ? names.join(",") : undefined, tag_mode: names.length > 1 ? draft.tag_mode : undefined })}
              load={tagLoad}
            />
            {tagListOf(draft.tags).length > 1 && (
              <Choices label="Match" name={`${id}-tagmode`} value={draft.tag_mode ?? "all"} onChange={(m) => set({ tag_mode: m === "any" ? "any" : undefined })}>
                {[
                  { value: "all", label: "All of them" },
                  { value: "any", label: "Any of them" },
                ]}
              </Choices>
            )}
          </div>
          {people !== undefined &&
            (people.length === 0 && !draft.people ? (
              <div>
                <span className="mb-1.5 block text-sm font-medium">People</span>
                <p className="text-sm text-muted">{NO_NAMED_PEOPLE}</p>
              </div>
            ) : (
              <div className="flex flex-col gap-2">
                <PeoplePicker
                  label="People"
                  people={people}
                  value={peopleIdsOf(draft.people)}
                  max={MAX_FILTER_PEOPLE}
                  onChange={(ids) => set({ people: ids.length ? ids.join(",") : undefined })}
                />
                <p className="text-sm text-muted">{PEOPLE_FILTER_NOTE}</p>
              </div>
            ))}
          <Field label="Rating" htmlFor={`${id}-rating`}>
            <select
              id={`${id}-rating`}
              className={fieldCls}
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
          </Field>
          <Choices
            label="File type"
            name={`${id}-type`}
            value={draft.type ?? ""}
            onChange={(t) => set({ type: (t || undefined) as Filters["type"] })}
          >
            {[{ value: "", label: "Any" }, ...(["jpeg", "heic", "raw", "other"] as const).map((t) => ({ value: t, label: TYPE_LABEL[t] }))]}
          </Choices>
          <Field label="Text in the photo" htmlFor={`${id}-text`}>
            <select
              id={`${id}-text`}
              className={fieldCls}
              value={draft.has_text === undefined ? "" : String(draft.has_text)}
              onChange={(e) => set({ has_text: e.target.value === "" ? undefined : e.target.value === "true" })}
            >
              <option value="">Any</option>
              <option value="true">Has text (signs, receipts, screenshots){facets?.with_text !== undefined ? ` (${n(facets.with_text)})` : ""}</option>
              <option value="false">No text</option>
            </select>
          </Field>
          <Switch
            checked={draft.include_archived !== false}
            onChange={(on) => set({ include_archived: on ? undefined : false })}
            label="Include archived"
          />
        </Group>

        <Group title="Where and with what">
          <Field label="Place" htmlFor={`${id}-place`}>
            <select id={`${id}-place`} className={fieldCls} value={draft.place ?? ""} onChange={(e) => set({ place: e.target.value })}>
              <option value="">Anywhere</option>
              {(["country", "region", "city"] as const).map((kind) => {
                const group = places.filter((p) => p.kind === kind);
                return group.length === 0 ? null : (
                  <optgroup key={kind} label={kind === "country" ? "Countries" : kind === "region" ? "Regions" : "Cities"}>
                    {group.map((p) => (
                      <option key={`${kind}:${p.value}`} value={p.value}>
                        {label(p)}
                      </option>
                    ))}
                  </optgroup>
                );
              })}
            </select>
          </Field>
          <Field label="Location" htmlFor={`${id}-loc`}>
            <select
              id={`${id}-loc`}
              className={fieldCls}
              value={draft.has_location === undefined ? "" : String(draft.has_location)}
              onChange={(e) => set({ has_location: e.target.value === "" ? undefined : e.target.value === "true" })}
            >
              <option value="">Any</option>
              <option value="true">Has a location{facets ? ` (${n(facets.with_location)})` : ""}</option>
              <option value="false">No location{facets ? ` (${n(facets.total - facets.with_location)})` : ""}</option>
            </select>
          </Field>
          <Field label="Camera" htmlFor={`${id}-camera`}>
            <select id={`${id}-camera`} className={fieldCls} value={draft.camera ?? ""} onChange={(e) => set({ camera: e.target.value })}>
              <option value="">Any</option>
              {cameras.map((c) => (
                <option key={c.value} value={c.value}>
                  {label(c)}
                </option>
              ))}
            </select>
          </Field>
          {lenses.length > 0 && (
            <Field label="Lens" htmlFor={`${id}-lens`}>
              <select id={`${id}-lens`} className={fieldCls} value={draft.lens ?? ""} onChange={(e) => set({ lens: e.target.value })}>
                <option value="">Any</option>
                {lenses.map((l) => (
                  <option key={l.value} value={l.value}>
                    {label(l)}
                  </option>
                ))}
              </select>
            </Field>
          )}
          <Field label="Album" htmlFor={`${id}-album`}>
            <select id={`${id}-album`} className={fieldCls} value={draft.album ?? ""} onChange={(e) => set({ album: e.target.value })}>
              <option value="">Any</option>
              {draft.album && !albums.some((a) => a.id === draft.album) && <option value={draft.album}>(selected album)</option>}
              {albums.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.title} ({n(a.photo_count)})
                </option>
              ))}
            </select>
          </Field>
        </Group>
      </div>
    </Sheet>
  );
}

function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-3.5">
      <h3 className="text-xs font-medium tracking-wide text-muted uppercase">{title}</h3>
      {children}
    </div>
  );
}

function Field({ label, htmlFor, children }: { label: string; htmlFor: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <label htmlFor={htmlFor} className="mb-1.5 block text-sm font-medium">
        {label}
      </label>
      {children}
    </div>
  );
}

/** One of a few options as pills (real radio buttons underneath, so the keyboard and screen readers get a radio group). */
function Choices({
  label,
  name,
  value,
  onChange,
  children,
}: {
  label: string;
  name: string;
  value: string;
  onChange: (v: string) => void;
  children: readonly { value: string; label: string }[];
}) {
  return (
    <fieldset className="m-0 min-w-0 border-0 p-0">
      <legend className="mb-1.5 p-0 text-sm font-medium">{label}</legend>
      <div className="flex flex-wrap gap-2">
        {children.map((o) => (
          <label
            key={o.value || "any"}
            className={cn(
              "relative inline-flex h-10 cursor-pointer items-center rounded-full px-4 text-[0.9375rem] transition-colors",
              "has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent",
              value === o.value ? "bg-accent text-on-accent" : "bg-surface text-fg hover:bg-line/70",
            )}
          >
            <input type="radio" name={name} className="sr-only" checked={value === o.value} onChange={() => onChange(o.value)} />
            {o.label}
          </label>
        ))}
      </div>
    </fieldset>
  );
}
