import { Eye, EyeOff, FolderPlus, Merge, MoreHorizontal, Pencil, Search, Trash2, UserRound } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, type FaceSuggestion, type Person, type Scope, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { CoverPicker } from "../components/CoverPicker";
import { ConfirmFaceDialog } from "../components/FaceChips";
import { BlankFace, FaceCrop } from "../components/FaceCrop";
import { Enter, Loading, PageProblem, Rise, useTitleCut } from "../components/pageKit";
import { RenameSheet } from "../components/RenameSheet";
import { ScopePhotos, type Membership } from "../components/ScopePhotos";
import { FormError, inputClass, Modal, Note } from "../components/sheetForm";
import { useToast } from "../components/Toast";
import { ADD_A_NAME, FIND_A_PERSON, filterablePeople, matchingPeople, mergedMessage, nameProblem, personLabel, photosWord, removedMessage, suggestionQuestion } from "../peopleText";
import { PageHeader } from "../shell/PageHeader";
import { usePeopleEnabled } from "../usePeople";
import { Button } from "../ui/Button";
import { Menu, MenuItem, MenuSeparator } from "../ui/Menu";
import { NotFound } from "./NotFound";

/** How often the person's counts are read again, to notice photos the worker has grouped meanwhile. */
const LIVE_EVERY_MS = 15000;

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
      {renaming && (
        <RenameSheet
          initial={person.name ?? ""}
          title={person.name?.trim() ? "Rename" : ADD_A_NAME}
          problem={nameProblem}
          onSave={(n) => void saveName(n)}
          onCancel={() => setRenaming(false)}
        />
      )}
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
  const view = useMemo(() => ({ person: person.id }), [person.id]);
  const scope = useMemo<Scope>(() => ({ person: person.id }), [person.id]);
  const membership = useMemo<Membership>(
    () => ({
      label: "Not this person",
      title: `Not ${personLabel(person)}?`,
      body: (photos) => (
        <p>
          {personLabel(person)}’s face is taken out of {photosWord(photos)}. It will not be put back, or suggested, for this person. The photos stay in
          your library.
        </p>
      ),
      remove: async (ids) => (await api.removeFromPerson(person.id, { asset_ids: ids })).removed,
      removed: (photos) => removedMessage(photos, person),
      cover: async (asset) => {
        const faces = await api.photoFaces(asset);
        const mine = faces.items.find((f) => f.person?.id === person.id);
        if (!mine) throw new Error("That photo has no face of this person.");
        await onCover(mine.id);
      },
    }),
    [person, onCover],
  );
  return (
    <ScopePhotos
      view={view}
      scope={scope}
      changed={`${person.photos}/${person.faces}`}
      words={photosWord}
      slideshowTitle={personLabel(person)}
      empty={`Photos where ${personLabel(person)} is found appear here.`}
      hiddenWhere="More, Hidden"
      membership={membership}
      failure={failure}
      show={show}
      onChanged={onChanged}
      onSave={onSave}
    />
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
