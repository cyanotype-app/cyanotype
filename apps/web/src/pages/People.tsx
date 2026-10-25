import { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type Person } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { FaceCrop } from "../components/FaceCrop";
import { MergePeopleDialog } from "../components/MergePeopleDialog";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { ADD_A_NAME, badgeText, bulkMessage, cardLabel, pendingLabel, peopleWord, personLabel, photosWord, splitPeople } from "../peopleText";
import { usePeopleEnabled } from "../usePeople";
import { NotFound } from "./NotFound";

interface Picking {
  /** Selection mode: a card selects instead of opening. */
  selecting: boolean;
  selected: ReadonlySet<string>;
  toggle: (id: string) => void;
}

/**
 * One person: a round face, the name (or "Add a name") and how many photos. The whole card opens the person's page; while selecting it
 * selects the person instead. A named person with "Is this X?" suggestions waiting wears a small count in the corner.
 */
export function PersonCard({ person, picking }: { person: Person; picking?: Picking }) {
  const pending = person.pending ?? 0;
  const face = person.cover ? (
    <FaceCrop url={person.cover.preview_url} box={person.cover.box} className="person-card-face" />
  ) : (
    <span className="face-crop round person-card-face blank" aria-hidden="true" />
  );
  const body = (
    <>
      <span className="person-card-picture">
        {face}
        {pending > 0 && (
          <span className="person-card-badge" aria-hidden="true" title={pendingLabel(pending)}>
            {badgeText(pending)}
          </span>
        )}
        {picking?.selecting && <span className="person-card-check" aria-hidden="true" />}
      </span>
      <span className={`person-card-name${person.name?.trim() ? "" : " muted"}`}>{person.name?.trim() || ADD_A_NAME}</span>
      <span className="person-card-count muted small">{photosWord(person.photos)}</span>
    </>
  );
  if (picking?.selecting) {
    const on = picking.selected.has(person.id);
    return (
      <li>
        <button type="button" className={`person-card${on ? " selected" : ""}`} aria-pressed={on} aria-label={cardLabel(person)} onClick={() => picking.toggle(person.id)}>
          {body}
        </button>
      </li>
    );
  }
  return (
    <li>
      <Link to={`/people/${person.id}`} className="person-card" aria-label={cardLabel(person)}>
        {body}
      </Link>
    </li>
  );
}

/** The people found in the owner's photos: named people, then the unnamed ones (to name), then the hidden ones in a collapsed section. */
export function PeoplePage() {
  const enabled = usePeopleEnabled();
  if (enabled === null) {
    return (
      <p className="center muted" role="status">
        Loading…
      </p>
    );
  }
  return enabled ? <PeopleList /> : <NotFound />;
}

type Dialog = "merge" | "delete" | null;

function PeopleList() {
  const { setAnonymous } = useAuth();
  const { toast, show } = useToast();
  const [people, setPeople] = useState<Person[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [hiddenOpen, setHiddenOpen] = useState(false);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const fail = useCallback(
    (e: unknown) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not load your people.");
    },
    [setAnonymous],
  );
  const reload = useCallback(() => api.listPeople().then((r) => setPeople(r.items)), []);

  useEffect(() => {
    let live = true;
    api
      .listPeople()
      .then((r) => live && setPeople(r.items))
      .catch((e: unknown) => live && fail(e));
    return () => {
      live = false;
    };
  }, [fail]);

  const { named, unnamed, hidden } = useMemo(() => splitPeople(people ?? []), [people]);
  const all = useMemo(() => people ?? [], [people]);
  const chosen = useMemo(() => all.filter((p) => selected.has(p.id)), [all, selected]);

  const leaveSelecting = useCallback(() => {
    setSelecting(false);
    setSelected(new Set());
  }, []);
  const toggle = useCallback(
    (id: string) =>
      setSelected((cur) => {
        const next = new Set(cur);
        if (!next.delete(id)) next.add(id);
        return next;
      }),
    [],
  );
  // Escape clears the selection, then leaves selection mode; the dialogs own the keyboard while they are open.
  useEffect(() => {
    if (!selecting || dialog !== null) return;
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      if (selected.size > 0) setSelected(new Set());
      else leaveSelecting();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selecting, selected, dialog, leaveSelecting]);

  async function run(what: Parameters<typeof api.bulkPeople>[0], message: string) {
    setBusy(true);
    setDialogError(null);
    try {
      await api.bulkPeople(what);
      await reload();
      setDialog(null);
      leaveSelecting();
      show({ message });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else if (dialog !== null) setDialogError(e instanceof Error ? e.message : "Could not change the people.");
      else show({ tone: "error", message: `Couldn’t change the people. ${e instanceof Error ? e.message : ""}`.trim() });
      // A person may have been merged or deleted elsewhere meanwhile: look again.
      if (e instanceof ApiError && e.status === 404) void reload().then(leaveSelecting, () => undefined);
    } finally {
      setBusy(false);
    }
  }
  const ids = chosen.map((p) => p.id);
  const anyShown = chosen.some((p) => !p.hidden);
  const anyHidden = chosen.some((p) => p.hidden);
  const picking: Picking = { selecting, selected, toggle };

  return (
    <div className="people-page">
      <div className="page-head">
        <div>
          <h1>People</h1>
          {people !== null && people.length > 0 && <p className="muted small">{peopleWord(named.length + unnamed.length)}</p>}
        </div>
        {people !== null && people.length > 0 && (
          <button
            type="button"
            className="btn btn-quiet"
            aria-pressed={selecting}
            onClick={() => {
              if (selecting) leaveSelecting();
              else {
                setSelecting(true);
                setHiddenOpen(true);
              }
            }}
          >
            Select
          </button>
        )}
      </div>
      {selecting && (
        <SelectionBar
          count={selected.size}
          total={all.length}
          emptyLabel="Select people"
          onSelectAll={() => setSelected(new Set(all.map((p) => p.id)))}
          onClear={() => setSelected(new Set())}
          onDone={leaveSelecting}
        >
          <button
            type="button"
            className="btn btn-quiet"
            disabled={chosen.length < 2 || busy}
            title={chosen.length < 2 ? "Select two or more people to merge" : undefined}
            onClick={() => {
              setDialogError(null);
              setDialog("merge");
            }}
          >
            Merge
          </button>
          {(anyShown || chosen.length === 0) && (
            <button
              type="button"
              className="btn btn-quiet"
              disabled={!anyShown || busy}
              onClick={() => void run({ action: "hide", ids }, bulkMessage.hide(ids.length))}
            >
              Hide
            </button>
          )}
          {anyHidden && (
            <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => void run({ action: "show", ids }, bulkMessage.show(ids.length))}>
              Show
            </button>
          )}
          <button
            type="button"
            className="btn btn-quiet btn-danger-quiet"
            disabled={chosen.length === 0 || busy}
            onClick={() => {
              setDialogError(null);
              setDialog("delete");
            }}
          >
            Delete
          </button>
        </SelectionBar>
      )}
      {error && (
        <p className="center" role="alert">
          {error}
        </p>
      )}
      {!error && people === null && (
        <p className="center muted" role="status">
          Loading your people…
        </p>
      )}
      {people !== null && people.length === 0 && (
        <div className="empty">
          <h2>No people yet</h2>
          <p>
            Faces are found in the background, and a person appears once three photos show the same face. Only you can see this page; nothing here is
            shared.
          </p>
        </div>
      )}
      {named.length > 0 && (
        <ul className="people-grid" aria-label="People">
          {named.map((p) => (
            <PersonCard key={p.id} person={p} picking={picking} />
          ))}
        </ul>
      )}
      {unnamed.length > 0 && (
        <section className="people-unnamed" aria-labelledby="people-unnamed-title">
          <h2 id="people-unnamed-title">Unnamed ({unnamed.length})</h2>
          <p className="muted small">Open a person to give them a name. Only named people can be found in search.</p>
          <ul className="people-grid" aria-label="Unnamed people">
            {unnamed.map((p) => (
              <PersonCard key={p.id} person={p} picking={picking} />
            ))}
          </ul>
        </section>
      )}
      {hidden.length > 0 && (
        <details className="people-hidden" open={hiddenOpen} onToggle={(e) => setHiddenOpen(e.currentTarget.open)}>
          <summary>Hidden people ({hidden.length})</summary>
          <ul className="people-grid" aria-label="Hidden people">
            {hidden.map((p) => (
              <PersonCard key={p.id} person={p} picking={picking} />
            ))}
          </ul>
        </details>
      )}
      {dialog === "merge" && (
        <MergePeopleDialog
          people={chosen}
          busy={busy}
          error={dialogError}
          onCancel={() => setDialog(null)}
          onMerge={(into, name) =>
            void run(
              { action: "merge", ids, into: into.id, ...(name !== undefined ? { name } : {}) },
              bulkMessage.merge(ids.length, { name: name ?? into.name ?? chosen.find((p) => p.name?.trim())?.name ?? null }),
            )
          }
        />
      )}
      {dialog === "delete" && (
        <ConfirmDialog
          title={chosen.length === 1 ? `Delete ${personLabel(chosen[0]!)}?` : `Delete ${peopleWord(chosen.length)}?`}
          confirmLabel={chosen.length === 1 ? "Delete person" : "Delete people"}
          danger
          busy={busy}
          onCancel={() => setDialog(null)}
          onConfirm={() => void run({ action: "delete", ids }, bulkMessage.delete(ids.length))}
        >
          <p>
            {chosen.length === 1 ? "The person and their name go" : "The people and their names go"}. The photos stay in your library, and their faces become
            unnamed again; they may be grouped into new people later. To keep someone out of sight, hide them instead.
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
