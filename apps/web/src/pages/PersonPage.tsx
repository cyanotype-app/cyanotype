import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, isScope, type AssetListItem, type FaceSuggestion, type Person, type Scope, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { CoverPicker } from "../components/CoverPicker";
import { FaceCrop } from "../components/FaceCrop";
import { Lightbox } from "../components/Lightbox";
import { RateTools } from "../components/RateTools";
import { SelectionBar } from "../components/SelectionBar";
import { ShareDialog, shareTargetOf, type ShareTarget } from "../components/ShareDialog";
import { TagTools } from "../components/TagTools";
import { TimelineGrid } from "../components/TimelineGrid";
import { useToast } from "../components/Toast";
import { filterablePeople, mergedMessage, nameProblem, personLabel, photosWord, removedMessage, suggestionQuestion, ADD_A_NAME } from "../peopleText";
import { useSelection } from "../selection";
import { similarUrl } from "../searchText";
import { inOrder, runAround } from "../timeline";
import { PAGE, useTimeline } from "../useTimeline";
import { usePeopleEnabled } from "../usePeople";
import { archiveMessage } from "../viewText";
import { NotFound } from "./NotFound";

/** How often the person's counts are read again, to notice photos the worker has grouped meanwhile. */
const LIVE_EVERY_MS = 15000;
const THUMB_POLL_MS = 3000;
const THUMB_POLL_MAX = 40; // ticks per photo (about two minutes), then leave it to a reload
/** Photos the lightbox pages through around the current one (it re-centres on every step). */
const LIGHTBOX_RUN = 60;

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

/**
 * One person: their photos as a timeline (it follows the worker as it groups new photos), "Is this X?" suggestions, rename, hide, cover,
 * merge, delete, taking photos out ("Not this person"), and saving their photos as an album.
 */
function PersonView({ id }: { id: string }) {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const { toast, show } = useToast();
  const [person, setPerson] = useState<Person | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [dialog, setDialog] = useState<"merge" | "delete" | "cover" | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState<{ target: Target; count: number } | null>(null);
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
  // Live: the worker groups new photos into people in the background; read the counts again now and then while the page is open.
  useEffect(() => {
    const t = setInterval(() => {
      if (document.visibilityState === "hidden") return;
      api.getPerson(id).then(
        (p) => setPerson((cur) => (cur && cur.photos === p.photos && cur.faces === p.faces && cur.name === p.name && cur.hidden === p.hidden ? cur : p)),
        () => undefined,
      );
    }, LIVE_EVERY_MS);
    return () => clearInterval(t);
  }, [id]);

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
  async function chooseCover(faceId: string) {
    setPerson(await api.updatePerson(id, { cover_face_id: faceId }));
    setDialog(null);
    show({ message: "Cover changed" });
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
          <button type="button" className="btn btn-quiet" disabled={person.faces === 0} onClick={() => setDialog("cover")}>
            Change cover
          </button>
          <button type="button" className="btn btn-quiet" onClick={() => void toggleHidden()}>
            {person.hidden ? "Show" : "Hide"}
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            disabled={person.photos === 0}
            onClick={() => setSaving({ target: { scope: { person: id }, except: [] }, count: person.photos })}
          >
            Save as album
          </button>
          <Link to={`/search?people=${encodeURIComponent(id)}`} className="btn btn-quiet">
            Search with filters
          </Link>
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
          if (yes) setVersion((v) => v + 1);
          void load(); // the count of suggestions waiting changed either way
        }}
        failure={failure}
        show={show}
      />
      <PersonPhotos
        key={version}
        person={person}
        failure={failure}
        show={show}
        onSave={(target, count) => setSaving({ target, count })}
        onChanged={async () => {
          await load();
        }}
        onCover={async (faceId) => {
          setPerson(await api.updatePerson(id, { cover_face_id: faceId }));
        }}
      />
      {saving !== null && (
        <AddToAlbumDialog
          mode="save"
          ids={saving.target}
          count={saving.count}
          onClose={() => setSaving(null)}
          onDone={(r) => {
            const requested = saving.count;
            setSaving(null);
            show({ message: addedMessage(r, requested) });
          }}
        />
      )}
      {dialog === "cover" && <CoverPicker person={person} onChoose={chooseCover} onCancel={() => setDialog(null)} />}
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

/**
 * The person's photos as the library's timeline (month and day headers, the scrubber, "Select all" through the server-side
 * `{person}` scope): the same components as the library, over `/assets/people/{id}/timeline` and `/photos`. The page looks again when
 * the person's counts change (the worker is grouping new photos), so it stays current without a reload.
 */
function PersonPhotos({
  person,
  failure,
  show,
  onChanged,
  onCover,
  onSave,
}: {
  person: Person;
  failure: (e: unknown, what: string) => void;
  show: (t: { message: string; tone?: "error"; action?: { label: string; onClick: () => void } }) => void;
  onChanged: () => Promise<void>;
  onCover: (faceId: string) => Promise<void>;
  /** "Save as album": the photos to save (explicit ids, or the whole person) and how many that is. */
  onSave: (target: Target, count: number) => void;
}) {
  const navigate = useNavigate();
  const [open, setOpen] = useState<number | null>(null); // global index of the photo in the lightbox
  const [confirming, setConfirming] = useState<"not" | "trash" | null>(null);
  const [busy, setBusy] = useState(false);
  const [addIds, setAddIds] = useState<{ target: Target; count: number } | null>(null);
  const [shareTarget, setShareTarget] = useState<ShareTarget | null>(null);
  const selectedRef = useRef<ReadonlySet<string>>(new Set());
  const tl = useTimeline({ view: { person: person.id }, keep: () => selectedRef.current });
  const { index, loaded, ensure, refresh, remove, patch } = tl;
  const items = useMemo(() => inOrder(loaded), [loaded]);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, index.total);
  const scope = useMemo<Scope>(() => ({ person: person.id }), [person.id]);
  useEffect(() => {
    // Selecting everything must not pin every loaded page in memory: only an explicit selection is kept loaded.
    selectedRef.current = sel.all ? new Set() : sel.selected;
  }, [sel.selected, sel.all]);

  // New photos are grouped in the background: when the person's counts change, read the timeline again. Selecting would lose the
  // selection to the swap, so that waits until selection mode ends.
  const dirty = useRef(false);
  const selecting = sel.selecting;
  const refreshSoon = useCallback(() => {
    if (selectedRef.current.size > 0 || selecting) dirty.current = true;
    else void refresh();
  }, [refresh, selecting]);
  useEffect(() => {
    if (!selecting && dirty.current) {
      dirty.current = false;
      void refresh();
    }
  }, [selecting, refresh]);
  const seen = useRef(`${person.photos}/${person.faces}`);
  const now = `${person.photos}/${person.faces}`;
  useEffect(() => {
    if (seen.current === now) return;
    seen.current = now;
    refreshSoon();
  }, [now, refreshSoon]);

  const refreshItem = useCallback(
    async (id: string) => {
      try {
        const d = await api.getAsset(id);
        patch(new Set([id]), { thumb_url: d.thumb_url, thumbhash: d.thumbhash });
      } catch {
        /* leave the tile as is */
      }
    },
    [patch],
  );
  // Thumbnails appear once the worker is done: look again, lightly, for tiles that still have none.
  const pendingKey = items
    .filter((i) => !i.thumb_url)
    .map((i) => i.id)
    .join(",");
  useEffect(() => {
    if (!pendingKey) return;
    const ticks = new Map<string, number>();
    const t = setInterval(() => {
      for (const id of pendingKey.split(",").slice(0, 12)) {
        const n = (ticks.get(id) ?? 0) + 1;
        ticks.set(id, n);
        if (n <= THUMB_POLL_MAX) void refreshItem(id);
      }
    }, THUMB_POLL_MS);
    return () => clearInterval(t);
  }, [pendingKey, refreshItem]);

  const leaveView = useCallback(
    (ids: string[]) => {
      const left = index.total - remove(new Set(ids)).length;
      setOpen((o) => (o === null || left <= 0 ? null : Math.min(o, left - 1)));
    },
    [index.total, remove],
  );
  const selectedItems = useMemo(() => {
    const ids = new Set(sel.ids);
    return items.filter((i) => ids.has(i.id));
  }, [sel.ids, items]);

  const setFavorite = useCallback(
    async (targets: AssetListItem[], favorite: boolean) => {
      if (targets.length === 0) return;
      const ids = targets.map((t) => t.id);
      const was = targets.filter((t) => t.favorite === true).map((t) => t.id);
      patch(new Set(ids), { favorite });
      try {
        await api.favoriteAssets(ids, favorite);
      } catch (e) {
        patch(new Set(ids), { favorite: false });
        patch(new Set(was), { favorite: true });
        failure(e, favorite ? "Couldn’t add to favorites." : "Couldn’t remove from favorites.");
        return;
      }
      if (targets.length > 1) show({ message: `${targets.length} ${favorite ? "added to" : "removed from"} favorites` });
    },
    [patch, failure, show],
  );
  const favoriteOne = useCallback(
    (id: string, favorite: boolean) => {
      const it = items.find((i) => i.id === id);
      if (it) void setFavorite([it], favorite);
    },
    [items, setFavorite],
  );
  const rateOne = useCallback(
    async (it: AssetListItem, rating: number) => {
      const was = it.rating ?? 0;
      patch(new Set([it.id]), { rating });
      try {
        await api.rateAssets([it.id], rating);
      } catch (e) {
        patch(new Set([it.id]), { rating: was });
        failure(e, "Couldn’t rate.");
      }
    },
    [patch, failure],
  );
  // For the whole person the loaded photos say nothing about the rest: "Favorite" then adds the lot.
  const allSelectedFavorite = !sel.all && selectedItems.length > 0 && selectedItems.every((i) => i.favorite === true);
  const favoriteSelected = useCallback(async () => {
    if (sel.all) {
      const target = sel.target(scope);
      sel.setSelecting(false);
      setBusy(true);
      try {
        const r = await api.favoriteAssets(target, true);
        void refresh();
        show({ message: `${r.count.toLocaleString("en-US")} added to favorites` });
      } catch (e) {
        failure(e, "Couldn’t add to favorites.");
      } finally {
        setBusy(false);
      }
      return;
    }
    const value = !allSelectedFavorite;
    sel.setSelecting(false);
    await setFavorite(selectedItems, value);
  }, [sel, scope, selectedItems, allSelectedFavorite, setFavorite, refresh, show, failure]);

  const undoTrash = useCallback(
    async (target: Target, count: number) => {
      try {
        await api.restoreAssets(target);
        void refresh();
        void onChanged();
        show({ message: `${count.toLocaleString("en-US")} restored` });
      } catch (e) {
        failure(e, "Couldn’t undo. They are still in the trash.");
      }
    },
    [refresh, onChanged, show, failure],
  );
  const trashSelected = useCallback(async () => {
    const target = sel.target(scope);
    const ids = [...sel.ids];
    sel.setSelecting(false);
    setBusy(true);
    try {
      if (!isScope(target)) leaveView(ids);
      const r = await api.trashAssets(target);
      if (isScope(target)) {
        setOpen(null);
        void refresh();
      }
      void onChanged();
      const back: Target = isScope(target) ? (r.batch ? { scope: { trash_batch: r.batch }, except: [] } : []) : target;
      show({
        message: `${r.count.toLocaleString("en-US")} moved to trash`,
        action: { label: "Undo", onClick: () => void undoTrash(back, r.count) },
      });
    } catch (e) {
      void refresh();
      failure(e, "Couldn’t move to trash.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, leaveView, refresh, onChanged, show, failure, undoTrash]);
  const trashOne = useCallback(
    async (it: AssetListItem) => {
      leaveView([it.id]);
      try {
        await api.trashAssets([it.id]);
      } catch (e) {
        void refresh();
        failure(e, "Couldn’t move to trash.");
        return;
      }
      void onChanged();
      show({ message: "1 moved to trash", action: { label: "Undo", onClick: () => void undoTrash([it.id], 1) } });
    },
    [leaveView, refresh, onChanged, show, failure, undoTrash],
  );

  // Archived photos stay on the person's page (flagged); hidden ones leave it.
  const archiving = sel.all || !selectedItems.every((i) => i.archived === true);
  const archiveSelected = useCallback(async () => {
    const target = sel.target(scope);
    const ids = [...sel.ids];
    sel.setSelecting(false);
    setBusy(true);
    try {
      const r = await api.archiveAssets(target, archiving);
      if (isScope(target)) void refresh();
      else patch(new Set(ids), { archived: archiving });
      show({ message: archiveMessage(r.count, archiving) });
    } catch (e) {
      failure(e, archiving ? "Couldn’t archive." : "Couldn’t restore.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, archiving, refresh, patch, show, failure]);
  const hideSelected = useCallback(async () => {
    const target = sel.target(scope);
    const ids = isScope(target) ? [] : [...sel.ids];
    sel.setSelecting(false);
    if (ids.length > 0) leaveView(ids);
    else setOpen(null);
    setBusy(true);
    try {
      const r = await api.hideAssets(target, true);
      if (ids.length === 0) void refresh();
      void onChanged();
      show({ message: `${r.count.toLocaleString("en-US")} hidden. Find ${r.count === 1 ? "it" : "them"} under More, Hidden.` });
    } catch (e) {
      void refresh();
      failure(e, "Couldn’t hide.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, leaveView, refresh, onChanged, show, failure]);

  async function notThisPerson() {
    const ids = sel.ids;
    setBusy(true);
    try {
      const r = await api.removeFromPerson(person.id, { asset_ids: ids });
      setConfirming(null);
      sel.setSelecting(false);
      show({ message: removedMessage(ids.length, person) });
      if (r.removed > 0) void refresh();
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

  // The lightbox pages through what is loaded around the open photo; keep that stocked as it moves.
  useEffect(() => {
    if (open !== null) ensure(Math.max(0, open - 5), open + PAGE, true);
  }, [open, ensure]);
  const run = open === null ? null : runAround(loaded, open, LIGHTBOX_RUN);

  // Escape clears the selection (then leaves selection mode); the lightbox and the dialogs own the keyboard while they are open.
  useEffect(() => {
    if (open !== null || confirming !== null || addIds !== null || shareTarget !== null) return;
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "Escape" && sel.selecting) {
        e.preventDefault();
        sel.escape();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, confirming, addIds, shareTarget, sel]);

  if (tl.status === "loading") {
    return (
      <p className="center muted" role="status">
        Loading the photos…
      </p>
    );
  }
  if (tl.status === "error") {
    return (
      <div className="empty" role="alert">
        <h2>Couldn’t load the photos</h2>
        <p className="muted">{tl.error}</p>
        <button type="button" className="btn btn-primary" onClick={tl.retry}>
          Try again
        </button>
      </div>
    );
  }
  return (
    <>
      {index.total === 0 ? (
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
        <SelectionBar count={sel.count} total={index.total} onSelectAll={sel.selectAll} onClear={sel.clear} onDone={() => sel.setSelecting(false)}>
          <button type="button" className="btn btn-quiet" disabled={sel.count === 0 || busy} onClick={() => void favoriteSelected()}>
            {allSelectedFavorite ? "Unfavorite" : "Favorite"}
          </button>
          <RateTools
            target={sel.target(scope)}
            count={sel.count}
            disabled={sel.count === 0 || busy}
            onDone={(_r, _n, message) => {
              sel.setSelecting(false);
              void refresh();
              show({ message });
            }}
            onError={(e) => failure(e, "Couldn’t rate.")}
          />
          <TagTools
            target={sel.target(scope)}
            count={sel.count}
            disabled={sel.count === 0 || busy}
            onDone={(message) => {
              sel.setSelecting(false);
              void refresh();
              show({ message });
            }}
          />
          <button type="button" className="btn btn-quiet" disabled={sel.count === 0 || busy} onClick={() => void archiveSelected()}>
            {archiving ? "Archive" : "Restore"}
          </button>
          <button type="button" className="btn btn-quiet" disabled={sel.count === 0 || busy} aria-label="Hide photos" onClick={() => void hideSelected()}>
            Hide
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sel.count === 0 || busy}
            onClick={() => setAddIds({ target: sel.target(scope), count: sel.count })}
          >
            Add to album
          </button>
          <button type="button" className="btn btn-quiet" disabled={sel.count === 0 || busy} onClick={() => onSave(sel.target(scope), sel.count)}>
            Save as album
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sel.count === 0 || busy}
            onClick={() => setShareTarget(shareTargetOf(sel.target(scope), sel.count))}
          >
            Share
          </button>
          <button type="button" className="btn btn-quiet" disabled={sel.count !== 1} onClick={() => void chooseCover()}>
            Use as cover
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sel.count === 0 || busy || sel.all}
            title={sel.all ? "Choose the photos one by one to take them out" : undefined}
            onClick={() => setConfirming("not")}
          >
            Not this person
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={sel.count === 0 || busy}
            onClick={() => (sel.all ? setConfirming("trash") : void trashSelected())}
          >
            Move to trash
          </button>
        </SelectionBar>
      )}
      {index.total > 0 && (
        <TimelineGrid
          index={index}
          loaded={loaded}
          onRange={ensure}
          onOpen={setOpen}
          onThumbError={(id) => void refreshItem(id)}
          selecting={sel.selecting}
          selected={sel.selected}
          onSelect={sel.select}
          onLongPress={sel.begin}
          onFavorite={favoriteOne}
        />
      )}
      {index.total > 0 && (
        <p className="muted center small" role="status">
          {photosWord(index.total)}
        </p>
      )}
      {tl.error && (
        <p className="center" role="alert">
          {tl.error}{" "}
          <button type="button" className="btn btn-quiet" onClick={tl.retry}>
            Retry
          </button>
        </p>
      )}
      {confirming === "not" && (
        <ConfirmDialog
          title={`Not ${personLabel(person)}?`}
          confirmLabel="Not this person"
          danger
          busy={busy}
          onCancel={() => setConfirming(null)}
          onConfirm={() => void notThisPerson()}
        >
          <p>
            {personLabel(person)}’s face is taken out of {photosWord(sel.count)}. It will not be put back, or suggested, for this person. The photos stay in
            your library.
          </p>
        </ConfirmDialog>
      )}
      {confirming === "trash" && (
        <ConfirmDialog
          title={`Move ${photosWord(sel.count)} to trash?`}
          confirmLabel="Move to trash"
          onCancel={() => setConfirming(null)}
          onConfirm={() => {
            setConfirming(null);
            void trashSelected();
          }}
        >
          <p>You can restore {sel.count === 1 ? "it" : "them"} from the trash.</p>
        </ConfirmDialog>
      )}
      {addIds !== null && (
        <AddToAlbumDialog
          ids={addIds.target}
          count={addIds.count}
          onClose={() => setAddIds(null)}
          onDone={(r) => {
            const requested = addIds.count;
            setAddIds(null);
            sel.setSelecting(false);
            show({ message: addedMessage(r, requested) });
          }}
        />
      )}
      {shareTarget !== null && <ShareDialog target={shareTarget} onClose={() => setShareTarget(null)} />}
      {open !== null && run && (
        <Lightbox
          items={run.items}
          index={open - run.start}
          offset={run.start}
          total={index.total}
          onIndexChange={(rel) => setOpen(run.start + rel)}
          onClose={() => setOpen(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onTrash={(it) => void trashOne(it)}
          onAddToAlbum={(it) => setAddIds({ target: [it.id], count: 1 })}
          onShare={(it) => setShareTarget({ kind: "photos", ids: [it.id] })}
          onFavorite={(it, favorite) => void setFavorite([it], favorite)}
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
        // Only named, shown people: an unnamed or hidden one is not a menu choice.
        const list = filterablePeople(r.items).filter((p) => p.id !== from.id);
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
        {others !== null && others.length === 0 && <p className="muted small">There is no named person to merge into. Name them first.</p>}
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
