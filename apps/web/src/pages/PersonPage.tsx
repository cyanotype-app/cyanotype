import { CheckSquare, Eye, EyeOff, FolderPlus, Images, Merge, MoreHorizontal, Pencil, Play, Search, Trash2, TriangleAlert, UserRound } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useRef, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, isScope, type AssetListItem, type FaceSuggestion, type Person, type Scope, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { CoverPicker } from "../components/CoverPicker";
import { ConfirmFaceDialog } from "../components/FaceChips";
import { BlankFace, FaceCrop } from "../components/FaceCrop";
import { Lightbox } from "../components/Lightbox";
import { Empty, Enter, InlineError, Loading, PageProblem, Rise, useTitleCut } from "../components/pageKit";
import { RateTools } from "../components/RateTools";
import { ZipButton } from "../components/ZipButton";
import { SelectionBar } from "../components/SelectionBar";
import { ShareDialog, shareTargetOf, type ShareTarget } from "../components/ShareDialog";
import { FormError, inputClass, Modal, Note, TextInput } from "../components/sheetForm";
import { TagTools } from "../components/TagTools";
import { TimelineGrid } from "../components/TimelineGrid";
import { useToast } from "../components/Toast";
import { ADD_A_NAME, FIND_A_PERSON, filterablePeople, matchingPeople, mergedMessage, nameProblem, personLabel, photosWord, removedMessage, suggestionQuestion } from "../peopleText";
import { useSelection } from "../selection";
import { similarUrl } from "../searchText";
import { PageHeader } from "../shell/PageHeader";
import { inOrder, runAround } from "../timeline";
import { PAGE, useTimeline } from "../useTimeline";
import { usePeopleEnabled } from "../usePeople";
import { archiveMessage } from "../viewText";
import { Button, buttonVariants } from "../ui/Button";
import { Menu, MenuItem, MenuSeparator } from "../ui/Menu";
import { NotFound } from "./NotFound";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { scopeSource } from "../slideshow/sources";

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
      <>
        <PageHeader title="Person" back={{ to: "/people", label: "People" }} />
        <Loading>Loading…</Loading>
      </>
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
  const nameCut = useTitleCut(person ? personLabel(person) : "");

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
      <PageProblem
        title="Couldn’t load the person"
        message={problem}
        back={{ to: "/people", label: "People" }}
        action={
          <Button asChild variant="primary">
            <Link to="/people">All people</Link>
          </Button>
        }
      />
    );
  }
  if (missing) {
    return (
      <PageProblem
        title="Person not found"
        message="They may have been merged into someone else or deleted."
        back={{ to: "/people", label: "People" }}
        action={
          <Button asChild variant="primary">
            <Link to="/people">All people</Link>
          </Button>
        }
      />
    );
  }
  if (!person) {
    return (
      <>
        <PageHeader title="Person" back={{ to: "/people", label: "People" }} />
        <Loading>Loading the person…</Loading>
      </>
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
    <>
      <PageHeader
        title={personLabel(person)}
        back={{ to: "/people", label: "People" }}
        actions={
          <Menu
            label="Person"
            trigger={
              <Button size="icon" variant="ghost" aria-label="More">
                <MoreHorizontal />
              </Button>
            }
          >
            <MenuItem onSelect={() => setRenaming(true)}>
              <Pencil />
              {person.name ? "Rename" : ADD_A_NAME}
            </MenuItem>
            <MenuItem disabled={person.faces === 0} onSelect={() => setDialog("cover")}>
              <UserRound />
              Change cover
            </MenuItem>
            <MenuItem onSelect={() => void toggleHidden()}>
              {person.hidden ? <Eye /> : <EyeOff />}
              {person.hidden ? "Show" : "Hide"}
            </MenuItem>
            <MenuItem disabled={person.photos === 0} onSelect={() => setSaving({ target: { scope: { person: id }, except: [] }, count: person.photos })}>
              <FolderPlus />
              Save as album
            </MenuItem>
            <MenuItem asChild>
              <Link to={`/search?people=${encodeURIComponent(id)}`}>
                <Search />
                Search with filters
              </Link>
            </MenuItem>
            <MenuItem onSelect={() => setDialog("merge")}>
              <Merge />
              Merge into…
            </MenuItem>
            <MenuSeparator />
            <MenuItem danger onSelect={() => setDialog("delete")}>
              <Trash2 />
              Delete person
            </MenuItem>
          </Menu>
        }
      />
      <Enter className="mx-auto flex max-w-[96rem] items-center gap-4 px-4 pt-2 pb-3 md:gap-6 md:px-8">
        <button
          type="button"
          className="size-24 shrink-0 rounded-full outline-offset-2 transition-transform duration-150 ease-soft hover:scale-[1.03] active:scale-[0.97] disabled:pointer-events-none md:size-28"
          aria-label="Change cover"
          disabled={person.faces === 0}
          onClick={() => setDialog("cover")}
        >
          {person.cover ? <FaceCrop url={person.cover.preview_url} box={person.cover.box} /> : <BlankFace />}
        </button>
        <div className="flex min-w-0 flex-col items-start gap-2">
          {nameCut && <p className="text-[1.25rem] leading-snug font-semibold tracking-tight text-pretty [overflow-wrap:anywhere]">{personLabel(person)}</p>}
          <p className="text-[1.0625rem] font-semibold tracking-tight">{photosWord(person.photos)}</p>
          {person.hidden && <p className="text-sm text-muted">Hidden from the People page and from photos</p>}
          {!person.name?.trim() && (
            <Button variant="primary" size="sm" onClick={() => setRenaming(true)}>
              <Pencil />
              {ADD_A_NAME}
            </Button>
          )}
        </div>
      </Enter>
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
      {renaming && <RenameSheet initial={person.name ?? ""} onSave={(n) => void saveName(n)} onCancel={() => setRenaming(false)} />}
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
        <ConfirmSheet title={`Delete ${personLabel(person)}?`} confirmLabel="Delete person" danger busy={busy} onCancel={() => setDialog(null)} onConfirm={() => void remove()}>
          <p>
            The person and their name go. The photos stay in your library, and their faces become unnamed again; they may be grouped into a new person
            later. To keep someone out of sight, hide them instead.
          </p>
          {dialogError && <FormError>{dialogError}</FormError>}
        </ConfirmSheet>
      )}
      {toast}
    </>
  );
}

/** Rename, or give a name: a sheet with one field. A blank name clears it. */
function RenameSheet({ initial, onSave, onCancel }: { initial: string; onSave: (name: string) => void; onCancel: () => void }) {
  const id = useId();
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  function submit(e: FormEvent) {
    e.preventDefault();
    const p = nameProblem(value);
    if (p) setError(p);
    else onSave(value);
  }
  return (
    <Modal
      onClose={onCancel}
      title={initial.trim() ? "Rename" : ADD_A_NAME}
      size="sm"
      footer={
        <>
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id}>
            Save
          </Button>
        </>
      }
    >
      <form id={id} className="flex flex-col gap-3" onSubmit={submit}>
        <label htmlFor={`${id}-name`} className="sr-only">
          Name
        </label>
        <TextInput
          id={`${id}-name`}
          data-autofocus=""
          placeholder="Name"
          value={value}
          maxLength={100}
          onChange={(e) => {
            setValue(e.target.value);
            setError(null);
          }}
          aria-invalid={error ? true : undefined}
        />
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
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
  show: (t: { message: string; tone?: "error"; action?: { label: string; onClick: () => void } }) => void;
}) {
  const [items, setItems] = useState<FaceSuggestion[]>([]);
  const [looking, setLooking] = useState<FaceSuggestion | null>(null);
  const [again, setAgain] = useState(0);
  useEffect(() => {
    let live = true;
    api.personSuggestions(person.id).then(
      (r) => live && setItems(r.items),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [person.id, again]);
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
  /** "Not a face": the detection is dropped from the list (and every count), with a way back. */
  async function notAFace(s: FaceSuggestion) {
    try {
      await api.markNotAFace(s.face_id);
      setItems((cur) => cur.filter((x) => x.face_id !== s.face_id));
      show({
        message: "Marked as not a face",
        action: {
          label: "Undo",
          onClick: () => {
            api.unmarkNotAFace(s.face_id).then(
              () => setAgain((n) => n + 1),
              (e: unknown) => failure(e, "Couldn’t undo that."),
            );
          },
        },
      });
    } catch (e) {
      failure(e, "Couldn’t save your answer.");
    }
  }
  if (items.length === 0) return null;
  return (
    <section className="mx-auto max-w-[96rem] px-4 pt-3 pb-2 md:px-8" aria-label="Suggestions">
      <h2 className="mb-3 text-[1.0625rem] font-semibold tracking-tight [overflow-wrap:anywhere]">{suggestionQuestion(person)}</h2>
      <ul className="grid grid-cols-2 gap-3 sm:grid-cols-4 md:grid-cols-[repeat(auto-fill,minmax(9rem,1fr))]">
        {items.map((s) => (
          <Rise key={s.face_id} className="flex flex-col gap-2 rounded-card bg-surface p-2">
            <button
              type="button"
              className="block overflow-hidden rounded-[0.6rem] transition-transform duration-150 ease-soft active:scale-[0.98]"
              aria-label="See the whole photo"
              onClick={() => setLooking(s)}
            >
              <FaceCrop url={s.preview_url} box={s.box} round={false} />
            </button>
            <span className="grid grid-cols-2 gap-1.5">
              <Button variant="primary" size="sm" aria-label={`Yes, this is ${personLabel(person)}`} onClick={() => void answer(s, true)}>
                Yes
              </Button>
              <Button variant="secondary" size="sm" className="bg-card" aria-label={`No, this is not ${personLabel(person)}`} onClick={() => void answer(s, false)}>
                No
              </Button>
            </span>
          </Rise>
        ))}
      </ul>
      {looking && (
        <ConfirmFaceDialog
          person={person}
          previewUrl={looking.preview_url}
          box={looking.box}
          onCancel={() => setLooking(null)}
          onYes={async () => {
            await answer(looking, true);
            setLooking(null);
          }}
          onNo={async () => {
            await answer(looking, false);
            setLooking(null);
          }}
          onNotAFace={async () => {
            await notAFace(looking);
            setLooking(null);
          }}
        />
      )}
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
  const slideshow = useSlideshow();
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

  if (tl.status === "loading") return <Loading>Loading the photos…</Loading>;
  if (tl.status === "error") {
    return (
      <Empty
        icon={TriangleAlert}
        title="Couldn’t load the photos"
        action={
          <Button variant="primary" onClick={tl.retry}>
            Try again
          </Button>
        }
      >
        {tl.error}
      </Empty>
    );
  }
  const quiet = (label: string, onClick: () => void, opts: { disabled?: boolean; title?: string; aria?: string } = {}) => (
    <Button variant="quiet" size="sm" disabled={opts.disabled} title={opts.title} aria-label={opts.aria} onClick={onClick}>
      {label}
    </Button>
  );
  return (
    <>
      {index.total === 0 ? (
        <Empty icon={Images} title="No photos">
          Photos where {personLabel(person)} is found appear here.
        </Empty>
      ) : (
        <div className="mx-auto flex max-w-[96rem] flex-wrap items-center gap-2 px-4 py-3 md:px-8">
          {slideshow && (
            <Button variant="secondary" size="sm" onClick={() => slideshow.start(scopeSource(scope, personLabel(person)))}>
              <Play />
              Slideshow
            </Button>
          )}
          <Button variant="secondary" size="sm" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
            <CheckSquare />
            Select
          </Button>
        </div>
      )}
      {sel.selecting && (
        <div className="mx-auto max-w-[96rem] px-4 md:px-8">
          <SelectionBar count={sel.count} total={index.total} onSelectAll={sel.selectAll} onClear={sel.clear} onDone={() => sel.setSelecting(false)}>
            {quiet(allSelectedFavorite ? "Unfavorite" : "Favorite", () => void favoriteSelected(), { disabled: sel.count === 0 || busy })}
            <ZipButton
              className={buttonVariants({ variant: "quiet", size: "sm" })}
              target={sel.target(scope)}
              disabled={sel.count === 0}
              onStarted={(message) => show({ message })}
              onError={(e) => failure(e, "Couldn’t download.")}
            />
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
            {quiet(archiving ? "Archive" : "Restore", () => void archiveSelected(), { disabled: sel.count === 0 || busy })}
            {quiet("Hide", () => void hideSelected(), { disabled: sel.count === 0 || busy, aria: "Hide photos" })}
            {quiet("Add to album", () => setAddIds({ target: sel.target(scope), count: sel.count }), { disabled: sel.count === 0 || busy })}
            {quiet("Save as album", () => onSave(sel.target(scope), sel.count), { disabled: sel.count === 0 || busy })}
            {quiet("Share", () => setShareTarget(shareTargetOf(sel.target(scope), sel.count)), { disabled: sel.count === 0 || busy })}
            {quiet("Use as cover", () => void chooseCover(), { disabled: sel.count !== 1 })}
            {quiet("Not this person", () => setConfirming("not"), {
              disabled: sel.count === 0 || busy || sel.all,
              title: sel.all ? "Choose the photos one by one to take them out" : undefined,
            })}
            <Button variant="primary" size="sm" disabled={sel.count === 0 || busy} onClick={() => (sel.all ? setConfirming("trash") : void trashSelected())}>
              Move to trash
            </Button>
          </SelectionBar>
        </div>
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
        <p className="py-4 text-center text-sm text-muted" role="status">
          {photosWord(index.total)}
        </p>
      )}
      {tl.error && <InlineError onRetry={tl.retry}>{tl.error}</InlineError>}
      {confirming === "not" && (
        <ConfirmSheet
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
        </ConfirmSheet>
      )}
      {confirming === "trash" && (
        <ConfirmSheet
          title={`Move ${photosWord(sel.count)} to trash?`}
          confirmLabel="Move to trash"
          onCancel={() => setConfirming(null)}
          onConfirm={() => {
            setConfirming(null);
            void trashSelected();
          }}
        >
          <p>You can restore {sel.count === 1 ? "it" : "them"} from the trash.</p>
        </ConfirmSheet>
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
  const id = useId();
  const [others, setOthers] = useState<Person[] | null>(null);
  const [into, setInto] = useState("");
  const [typed, setTyped] = useState("");
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
  const target = others?.find((p) => p.id === into);
  const shown = matchingPeople(others ?? [], typed, [into]);
  return (
    <Modal
      onClose={onCancel}
      title={`Merge ${personLabel(from)} into…`}
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id} disabled={busy || !target}>
            Merge
          </Button>
        </>
      }
    >
      <form
        id={id}
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (target) onMerge(target);
        }}
      >
        {others !== null && others.length > 6 && (
          <>
            <label htmlFor={`${id}-find`} className="sr-only">
              {FIND_A_PERSON}
            </label>
            <input
              id={`${id}-find`}
              className={inputClass}
              type="search"
              placeholder={FIND_A_PERSON}
              value={typed}
              maxLength={100}
              onChange={(e) => setTyped(e.target.value)}
            />
          </>
        )}
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy || !others?.length}>
          <legend className="sr-only">Merge into</legend>
          <div className="flex max-h-72 flex-col overflow-y-auto">
            {shown.map((p) => (
              <label key={p.id} className="flex min-h-14 cursor-pointer items-center gap-3 rounded-card px-1.5 hover:bg-surface has-[:checked]:bg-surface has-[:disabled]:cursor-default">
                <input
                  type="radio"
                  name={`${id}-into`}
                  className="size-5 shrink-0 accent-accent"
                  checked={p.id === into}
                  data-autofocus={p.id === into ? "" : undefined}
                  onChange={() => setInto(p.id)}
                />
                {p.cover ? <FaceCrop url={p.cover.preview_url} box={p.cover.box} className="size-11 shrink-0" /> : <BlankFace className="size-11 shrink-0" />}
                <span className="min-w-0">
                  <span className="block truncate text-[0.9375rem]">{personLabel(p)}</span>
                  <span className="block text-[0.8125rem] text-muted">{photosWord(p.photos)}</span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>
        {others !== null && others.length === 0 && <Note>There is no named person to merge into. Name them first.</Note>}
        <Note>
          All of {personLabel(from)}’s photos go to the other person, and {personLabel(from)} is deleted. The other person keeps their name.
        </Note>
        {(error ?? loadError) && <FormError>{error ?? loadError}</FormError>}
      </form>
    </Modal>
  );
}
