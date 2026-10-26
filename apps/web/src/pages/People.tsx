import { Check, CheckSquare, ChevronRight, Users } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type Person } from "../api";
import { useAuth } from "../auth";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { BlankFace, FaceCrop } from "../components/FaceCrop";
import { MergePeopleDialog } from "../components/MergePeopleDialog";
import { Empty, InlineError, Loading, Rise } from "../components/pageKit";
import { FormError } from "../components/sheetForm";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { ADD_A_NAME, badgeText, bulkMessage, cardLabel, pendingLabel, peopleWord, personLabel, photosWord, splitPeople } from "../peopleText";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
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
export function PersonCard({ person, picking, index = 0 }: { person: Person; picking?: Picking; index?: number }) {
  const pending = person.pending ?? 0;
  const named = !!person.name?.trim();
  const on = picking?.selected.has(person.id) ?? false;
  const body = (
    <>
      <span className={cn("relative mx-auto block w-full max-w-36 rounded-full transition-[box-shadow,transform] duration-150 ease-soft group-active:scale-[0.97]", on && "ring-[3px] ring-accent ring-offset-2 ring-offset-bg")}>
        {person.cover ? <FaceCrop url={person.cover.preview_url} box={person.cover.box} /> : <BlankFace />}
        {pending > 0 && (
          <span
            data-badge="" className="absolute -top-0.5 -right-0.5 flex min-w-5 items-center justify-center rounded-full bg-accent px-1.5 text-[0.6875rem] leading-5 font-semibold text-on-accent ring-2 ring-bg"
            aria-hidden="true"
            title={pendingLabel(pending)}
          >
            {badgeText(pending)}
          </span>
        )}
        {picking?.selecting && (
          <span
            className={cn(
              "absolute bottom-0.5 left-0.5 flex size-6 items-center justify-center rounded-full ring-2 ring-white/90",
              on ? "bg-accent text-on-accent" : "bg-black/35 text-transparent backdrop-blur-sm",
            )}
            aria-hidden="true"
          >
            <Check className="size-3.5" strokeWidth={3} />
          </span>
        )}
      </span>
      <span className={cn("mt-2 block truncate px-1 text-center text-[0.9375rem] leading-tight font-medium", !named && "font-normal text-muted")}>
        {person.name?.trim() || ADD_A_NAME}
      </span>
      <span className="block text-center text-[0.8125rem] text-muted">{photosWord(person.photos)}</span>
    </>
  );
  const cls = "group block w-full rounded-card px-1 pt-1 pb-1.5 text-fg no-underline outline-offset-2";
  if (picking?.selecting) {
    return (
      <Rise index={index}>
        <button type="button" className={cls} aria-pressed={on} aria-label={cardLabel(person)} onClick={() => picking.toggle(person.id)}>
          {body}
        </button>
      </Rise>
    );
  }
  return (
    <Rise index={index}>
      <Link to={`/people/${person.id}`} className={cls} aria-label={cardLabel(person)}>
        {body}
      </Link>
    </Rise>
  );
}

const gridClass = "grid grid-cols-3 gap-x-2 gap-y-4 sm:grid-cols-4 md:grid-cols-[repeat(auto-fill,minmax(8.5rem,1fr))] md:gap-x-4 md:gap-y-6";

/** The people found in the owner's photos: named people, then the unnamed ones (to name), then the hidden ones in a collapsed section. */
export function PeoplePage() {
  const enabled = usePeopleEnabled();
  if (enabled === null) {
    return (
      <>
        <PageHeader title="People" />
        <Loading>Loading…</Loading>
      </>
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
  const some = people !== null && people.length > 0;

  return (
    <>
      <PageHeader
        title="People"
        subtitle={some ? peopleWord(named.length + unnamed.length) : undefined}
        actions={
          some && (
            <Button
              size="icon"
              variant={selecting ? "secondary" : "ghost"}
              aria-label="Select"
              aria-pressed={selecting}
              onClick={() => {
                if (selecting) leaveSelecting();
                else {
                  setSelecting(true);
                  setHiddenOpen(true);
                }
              }}
            >
              <CheckSquare />
            </Button>
          )
        }
      />
      <div className="mx-auto max-w-[96rem] px-4 pt-2 pb-10 md:px-8">
        {selecting && (
          <SelectionBar
            count={selected.size}
            total={all.length}
            emptyLabel="Select people"
            onSelectAll={() => setSelected(new Set(all.map((p) => p.id)))}
            onClear={() => setSelected(new Set())}
            onDone={leaveSelecting}
          >
            <Button
              variant="quiet"
              size="sm"
              disabled={chosen.length < 2 || busy}
              title={chosen.length < 2 ? "Select two or more people to merge" : undefined}
              onClick={() => {
                setDialogError(null);
                setDialog("merge");
              }}
            >
              Merge
            </Button>
            {(anyShown || chosen.length === 0) && (
              <Button variant="quiet" size="sm" disabled={!anyShown || busy} onClick={() => void run({ action: "hide", ids }, bulkMessage.hide(ids.length))}>
                Hide
              </Button>
            )}
            {anyHidden && (
              <Button variant="quiet" size="sm" disabled={busy} onClick={() => void run({ action: "show", ids }, bulkMessage.show(ids.length))}>
                Show
              </Button>
            )}
            <Button
              variant="quiet"
              size="sm"
              className="text-danger hover:text-danger"
              disabled={chosen.length === 0 || busy}
              onClick={() => {
                setDialogError(null);
                setDialog("delete");
              }}
            >
              Delete
            </Button>
          </SelectionBar>
        )}
        {error && <InlineError>{error}</InlineError>}
        {!error && people === null && <Loading>Loading your people…</Loading>}
        {people !== null && people.length === 0 && (
          <Empty icon={Users} title="No people yet">
            Faces are found in the background, and a person appears once three photos show the same face. Only you can see this page; nothing here is
            shared.
          </Empty>
        )}
        {named.length > 0 && (
          <ul className={cn(gridClass, "pt-3")} aria-label="People">
            {named.map((p, i) => (
              <PersonCard key={p.id} person={p} picking={picking} index={i} />
            ))}
          </ul>
        )}
        {unnamed.length > 0 && (
          <section className="mt-10" aria-labelledby="people-unnamed-title">
            <h2 id="people-unnamed-title" className="text-[1.0625rem] font-semibold tracking-tight">
              Unnamed ({unnamed.length})
            </h2>
            <p className="mt-0.5 text-sm text-muted">Open a person to give them a name. Only named people can be found in search.</p>
            <ul className={cn(gridClass, "pt-3")} aria-label="Unnamed people">
              {unnamed.map((p, i) => (
                <PersonCard key={p.id} person={p} picking={picking} index={i} />
              ))}
            </ul>
          </section>
        )}
        {hidden.length > 0 && (
          <details data-hidden-people="" className="group/hidden mt-10" open={hiddenOpen} onToggle={(e) => setHiddenOpen(e.currentTarget.open)}>
            <summary className="flex h-11 cursor-pointer list-none items-center gap-1.5 rounded-full text-[1.0625rem] font-semibold tracking-tight select-none [&::-webkit-details-marker]:hidden">
              <ChevronRight className="size-4 text-muted transition-transform duration-200 group-open/hidden:rotate-90" aria-hidden="true" />
              Hidden people ({hidden.length})
            </summary>
            <ul className={cn(gridClass, "pt-3")} aria-label="Hidden people">
              {hidden.map((p, i) => (
                <PersonCard key={p.id} person={p} picking={picking} index={i} />
              ))}
            </ul>
          </details>
        )}
      </div>
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
        <ConfirmSheet
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
          {dialogError && <FormError>{dialogError}</FormError>}
        </ConfirmSheet>
      )}
      {toast}
    </>
  );
}
