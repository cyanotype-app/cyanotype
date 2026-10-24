import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, type FaceSuggestion, type Person } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { FaceCrop } from "../components/FaceCrop";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { mergedMessage, nameProblem, personLabel, photosWord, removedMessage, suggestionQuestion, ADD_A_NAME } from "../peopleText";
import { useSelection } from "../selection";
import { similarUrl } from "../searchText";
import { usePeopleEnabled } from "../usePeople";
import { useAssetPages } from "../useAssetPages";
import { NotFound } from "./NotFound";

const PAGE = 60;

export function PersonPage() {
  const { id = "" } = useParams();
  const enabled = usePeopleEnabled();
  if (enabled === null) {
    return (
      <p className="center muted" role="status">
        Loading…
      </p>
    );
  }
  return enabled ? <PersonView key={id} id={id} /> : <NotFound />;
}

/** One person: their photos, "Is this X?" suggestions, rename, hide, cover, merge, delete, and taking photos out ("Not this person"). */
function PersonView({ id }: { id: string }) {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const { toast, show } = useToast();
  const [person, setPerson] = useState<Person | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [dialog, setDialog] = useState<"merge" | "delete" | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState<number | null>(null);
  const [version, setVersion] = useState(0);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  const load = useCallback(async () => {
    try {
      setPerson(await api.getPerson(id));
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else if (e instanceof ApiError && e.status === 404) setMissing(true);
      else setProblem(e instanceof Error ? e.message : "Could not load the person.");
    }
  }, [id, setAnonymous]);
  useEffect(() => {
    void load();
  }, [load]);

  const fetchPage = useCallback(
    async (cursor: string | null) => {
      const r = await api.personPhotos(id, { cursor, limit: PAGE });
      return { items: r.items, next_cursor: r.next_cursor ?? null, next_before: null };
    },
    [id],
  );

  if (problem) {
    return (
      <div className="empty" role="alert">
        <h1>Couldn’t load the person</h1>
        <p className="muted">{problem}</p>
        <Link to="/people" className="btn btn-primary">
          All people
        </Link>
      </div>
    );
  }
  if (missing) {
    return (
      <div className="empty" role="alert">
        <h1>Person not found</h1>
        <p className="muted">They may have been merged into someone else or deleted.</p>
        <Link to="/people" className="btn btn-primary">
          All people
        </Link>
      </div>
    );
  }
  if (!person) {
    return (
      <p className="center muted" role="status">
        Loading the person…
      </p>
    );
  }

  async function saveName(name: string) {
    try {
      setPerson(await api.updatePerson(id, { name: name.trim() === "" ? null : name.trim() }));
      setRenaming(false);
    } catch (e) {
      failure(e, "Couldn’t save the name.");
    }
  }
  async function toggleHidden() {
    if (!person) return;
    try {
      setPerson(await api.updatePerson(id, { hidden: !person.hidden }));
      show({ message: person.hidden ? `${personLabel(person)} is shown again` : `${personLabel(person)} is hidden from the People page and from photos` });
    } catch (e) {
      failure(e, "Couldn’t change that.");
    }
  }
  async function merge(into: Person) {
    setBusy(true);
    setDialogError(null);
    try {
      const r = await api.mergePerson(id, into.id);
      show({ message: mergedMessage(into, r.faces) });
      navigate(`/people/${into.id}`, { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setDialogError(e instanceof Error ? e.message : "Could not merge.");
      setBusy(false);
    }
  }
  async function remove() {
    setBusy(true);
    try {
      await api.deletePerson(id);
      navigate("/people", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setDialogError(e instanceof Error ? e.message : "Could not delete the person.");
      setBusy(false);
    }
  }

  return (
    <div className="library person-page">
      <p className="small">
        <Link to="/people">‹ People</Link>
      </p>
      <div className="page-head album-head person-head">
        {person.cover ? (
          <FaceCrop url={person.cover.preview_url} box={person.cover.box} className="person-head-face" />
        ) : (
          <span className="face-crop round person-head-face blank" aria-hidden="true" />
        )}
        <div className="person-head-text">
          {renaming ? (
            <RenameForm initial={person.name ?? ""} onSave={(n) => void saveName(n)} onCancel={() => setRenaming(false)} />
          ) : (
            <>
              <h1 className="person-title">{personLabel(person)}</h1>
              <p className="muted small">
                {photosWord(person.photos)}
                {person.hidden && " · Hidden"}
              </p>
            </>
          )}
        </div>
        <div className="album-head-actions">
          {!renaming && (
            <button type="button" className="btn btn-quiet" onClick={() => setRenaming(true)}>
              {person.name ? "Rename" : ADD_A_NAME}
            </button>
          )}
          <button type="button" className="btn btn-quiet" onClick={() => void toggleHidden()}>
            {person.hidden ? "Show" : "Hide"}
          </button>
          <button type="button" className="btn btn-quiet" onClick={() => setDialog("merge")}>
            Merge into…
          </button>
          <button type="button" className="btn btn-quiet btn-danger-quiet" onClick={() => setDialog("delete")}>
            Delete person
          </button>
        </div>
      </div>
      <Suggestions
        key={`${id}-${version}`}
        person={person}
        onAnswered={(yes) => {
          if (yes) {
            setVersion((v) => v + 1);
            void load();
          }
        }}
        failure={failure}
        show={show}
      />
      <PersonPhotos
        key={version}
        person={person}
        fetchPage={fetchPage}
        open={open}
        setOpen={setOpen}
        failure={failure}
        show={show}
        onChanged={async () => {
          await load();
        }}
        onCover={async (faceId) => {
          setPerson(await api.updatePerson(id, { cover_face_id: faceId }));
        }}
      />
      {dialog === "merge" && <MergeDialog from={person} busy={busy} error={dialogError} onMerge={(p) => void merge(p)} onCancel={() => setDialog(null)} />}
      {dialog === "delete" && (
        <ConfirmDialog title={`Delete ${personLabel(person)}?`} confirmLabel="Delete person" danger busy={busy} onCancel={() => setDialog(null)} onConfirm={() => void remove()}>
          <p>
            The person and their name go. The photos stay in your library, and their faces become unnamed again; they may be grouped into a new person
            later. To keep someone out of sight, hide them instead.
          </p>
          {dialogError && (
            <p role="alert" className="form-error">
              {dialogError}
            </p>
          )}
        </ConfirmDialog>
      )}
      {toast}
    </div>
  );
}

function RenameForm({ initial, onSave, onCancel }: { initial: string; onSave: (name: string) => void; onCancel: () => void }) {
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  function submit(e: FormEvent) {
    e.preventDefault();
    const p = nameProblem(value);
    if (p) setError(p);
    else onSave(value);
  }
  return (
    <form className="person-rename" onSubmit={submit} onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <label htmlFor="person-name" className="sr-only">
        Name
      </label>
      <input
        id="person-name"
        className="text-input"
        placeholder="Name"
        value={value}
        maxLength={100}
        autoFocus
        onChange={(e) => setValue(e.target.value)}
        aria-invalid={error ? true : undefined}
      />
      <button type="submit" className="btn btn-primary">
        Save
      </button>
      <button type="button" className="btn btn-quiet" onClick={onCancel}>
        Cancel
      </button>
      {error && (
        <p role="alert" className="form-error">
          {error}
        </p>
      )}
    </form>
  );
}

/** "Is this X?": faces nobody has claimed that look like the person, each with Yes and No. */
function Suggestions({
  person,
  onAnswered,
  failure,
  show,
}: {
  person: Person;
  onAnswered: (yes: boolean) => void;
  failure: (e: unknown, what: string) => void;
  show: (t: { message: string; tone?: "error" }) => void;
}) {
  const [items, setItems] = useState<FaceSuggestion[]>([]);
  useEffect(() => {
    let live = true;
    api.personSuggestions(person.id).then(
      (r) => live && setItems(r.items),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [person.id]);
  async function answer(s: FaceSuggestion, yes: boolean) {
    try {
      if (yes) await api.assignFace(s.face_id, { person_id: person.id });
      else await api.rejectFace(s.face_id, person.id);
      setItems((cur) => cur.filter((x) => x.face_id !== s.face_id));
      if (yes) show({ message: `Added to ${personLabel(person)}` });
      onAnswered(yes);
    } catch (e) {
      failure(e, "Couldn’t save your answer.");
    }
  }
  if (items.length === 0) return null;
  return (
    <section className="person-suggestions" aria-label="Suggestions">
      <h2>{suggestionQuestion(person)}</h2>
      <ul className="suggestion-list">
        {items.map((s) => (
          <li key={s.face_id} className="suggestion">
            <FaceCrop url={s.preview_url} box={s.box} round={false} className="suggestion-face" />
            <span className="suggestion-actions">
              <button type="button" className="btn btn-primary" aria-label={`Yes, this is ${personLabel(person)}`} onClick={() => void answer(s, true)}>
                Yes
              </button>
              <button type="button" className="btn btn-quiet" aria-label={`No, this is not ${personLabel(person)}`} onClick={() => void answer(s, false)}>
                No
              </button>
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function PersonPhotos({
  person,
  fetchPage,
  open,
  setOpen,
  failure,
  show,
  onChanged,
  onCover,
}: {
  person: Person;
  fetchPage: (cursor: string | null) => Promise<{ items: import("../api").AssetListItem[]; next_cursor: string | null; next_before: null }>;
  open: number | null;
  setOpen: (i: number | null) => void;
  failure: (e: unknown, what: string) => void;
  show: (t: { message: string; tone?: "error" }) => void;
  onChanged: () => Promise<void>;
  onCover: (faceId: string) => Promise<void>;
}) {
  const navigate = useNavigate();
  const { items, done, started, error, loadMore, sentinel, refreshItem, reset } = useAssetPages(fetchPage);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, undefined);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const setFavorite = useCallback(
    async (id: string, favorite: boolean) => {
      refreshItem(id, { favorite });
      try {
        await api.favoriteAssets([id], favorite);
      } catch (e) {
        refreshItem(id, { favorite: !favorite });
        failure(e, "Couldn’t update the favorite.");
      }
    },
    [refreshItem, failure],
  );
  const rateOne = useCallback(
    async (it: { id: string; rating?: number }, rating: number) => {
      const was = it.rating ?? 0;
      refreshItem(it.id, { rating });
      try {
        await api.rateAssets([it.id], rating);
      } catch (e) {
        refreshItem(it.id, { rating: was });
        failure(e, "Couldn’t rate.");
      }
    },
    [refreshItem, failure],
  );
  async function notThisPerson() {
    const ids = sel.ids;
    setBusy(true);
    try {
      const r = await api.removeFromPerson(person.id, { asset_ids: ids });
      setConfirming(false);
      sel.setSelecting(false);
      show({ message: removedMessage(ids.length, person) });
      if (r.removed > 0) reset();
      void onChanged();
    } catch (e) {
      failure(e, "Couldn’t take the photos out.");
    } finally {
      setBusy(false);
    }
  }
  async function chooseCover() {
    const asset = sel.ids[0];
    if (!asset) return;
    try {
      const faces = await api.photoFaces(asset);
      const mine = faces.items.find((f) => f.person?.id === person.id);
      if (!mine) return show({ tone: "error", message: "That photo has no face of this person." });
      await onCover(mine.id);
      sel.setSelecting(false);
      show({ message: "Cover changed" });
    } catch (e) {
      failure(e, "Couldn’t change the cover.");
    }
  }

  if (!started) {
    return (
      <p className="center muted" role="status">
        Loading the photos…
      </p>
    );
  }
  return (
    <>
      {items.length === 0 ? (
        <div className="empty">
          <h2>No photos</h2>
          <p>Photos where {personLabel(person)} is found appear here.</p>
        </div>
      ) : (
        <div className="library-bar">
          <button type="button" className="btn btn-quiet" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
            Select
          </button>
        </div>
      )}
      {sel.selecting && (
        <SelectionBar count={sel.count} total={items.length} onSelectAll={sel.selectAll} onClear={sel.clear} onDone={() => sel.setSelecting(false)}>
          <button type="button" className="btn btn-quiet" disabled={sel.count !== 1} onClick={() => void chooseCover()}>
            Use as cover
          </button>
          <button type="button" className="btn btn-primary" disabled={sel.count === 0} onClick={() => setConfirming(true)}>
            Not this person
          </button>
        </SelectionBar>
      )}
      {items.length > 0 && (
        <Grid
          items={items}
          onOpen={setOpen}
          onThumbError={(i) => void api.getAsset(i).then((d) => refreshItem(i, { thumb_url: d.thumb_url, thumbhash: d.thumbhash }), () => undefined)}
          selecting={sel.selecting}
          selected={sel.selected}
          onSelect={sel.select}
          onLongPress={sel.begin}
          onFavorite={(id, favorite) => void setFavorite(id, favorite)}
        />
      )}
      <div ref={sentinel} className="sentinel" aria-hidden="true" />
      {error && (
        <p className="center" role="alert">
          {error}{" "}
          <button type="button" className="btn btn-quiet" onClick={() => void loadMore()}>
            Retry
          </button>
        </p>
      )}
      {!done && items.length > 0 && (
        <p className="center muted small" role="status">
          Loading more…
        </p>
      )}
      {confirming && (
        <ConfirmDialog
          title={`Not ${personLabel(person)}?`}
          confirmLabel="Not this person"
          danger
          busy={busy}
          onCancel={() => setConfirming(false)}
          onConfirm={() => void notThisPerson()}
        >
          <p>
            {personLabel(person)}’s face is taken out of {photosWord(sel.count)}. It will not be put back, or suggested, for this person. The photos stay in
            your library.
          </p>
        </ConfirmDialog>
      )}
      {open !== null && items.length > 0 && (
        <Lightbox
          items={items}
          index={Math.min(open, items.length - 1)}
          onIndexChange={(i) => {
            setOpen(i);
            if (i >= items.length - 3 && !done && !error) void loadMore();
          }}
          onClose={() => setOpen(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onFavorite={(it, favorite) => void setFavorite(it.id, favorite)}
          onRate={(it, n) => void rateOne(it, n)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
    </>
  );
}

/** Choose the person this one becomes part of. */
function MergeDialog({
  from,
  busy,
  error,
  onMerge,
  onCancel,
}: {
  from: Person;
  busy: boolean;
  error: string | null;
  onMerge: (into: Person) => void;
  onCancel: () => void;
}) {
  const [others, setOthers] = useState<Person[] | null>(null);
  const [into, setInto] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    api
      .listPeople()
      .then((r) => {
        if (!live) return;
        const list = r.items.filter((p) => p.id !== from.id);
        setOthers(list);
        setInto(list[0]?.id ?? "");
      })
      .catch((e: unknown) => live && setLoadError(e instanceof Error ? e.message : "Could not load your people."));
    return () => {
      live = false;
    };
  }, [from.id]);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);
  const target = others?.find((p) => p.id === into);
  return (
    <div className="modal-backdrop">
      <form
        className="modal card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="merge-person-title"
        onSubmit={(e) => {
          e.preventDefault();
          if (target) onMerge(target);
        }}
      >
        <h2 id="merge-person-title">Merge {personLabel(from)} into…</h2>
        <label htmlFor="merge-person-into" className="sr-only">
          Merge into
        </label>
        <select id="merge-person-into" className="text-input" value={into} onChange={(e) => setInto(e.target.value)} disabled={busy || !others?.length} autoFocus>
          {(others ?? []).map((p) => (
            <option key={p.id} value={p.id}>
              {personLabel(p)} ({photosWord(p.photos)})
            </option>
          ))}
        </select>
        {others !== null && others.length === 0 && <p className="muted small">There is no one else to merge into.</p>}
        <p className="muted small">All of {personLabel(from)}’s photos go to the other person, and {personLabel(from)} is deleted. The other person keeps their name.</p>
        {(error ?? loadError) && (
          <p role="alert" className="form-error">
            {error ?? loadError}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || !target}>
            Merge
          </button>
        </div>
      </form>
    </div>
  );
}
