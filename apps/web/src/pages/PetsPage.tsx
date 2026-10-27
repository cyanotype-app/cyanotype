import { FolderPlus, MoreHorizontal, PawPrint, Plus, Search, Tag, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api, ApiError, type NamedPet, type PetKind, type PetsSummary, type Scope, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { Empty, Enter, Loading } from "../components/pageKit";
import { ScopePhotos } from "../components/ScopePhotos";
import { useToast } from "../components/Toast";
import { NamedPetCard } from "../collections/cards";
import { ScrollRow } from "../collections/ScrollRow";
import { offeredKinds, PET_LABEL, PET_TAB, petCount, petKindOf, petPhotos, scanLine, scanning, scanPercent } from "../petsText";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { Segmented } from "../ui/Controls";
import { Menu, MenuItem } from "../ui/Menu";
import { usePetsEnabled } from "../usePets";
import { NotFound } from "./NotFound";

/** How often the counts are read again, to notice photos the worker has looked at meanwhile. */
const LIVE_EVERY_MS = 15000;

type Summary = { status: "loading" } | { status: "ready"; data: PetsSummary } | { status: "error"; message: string };

export function PetsPage() {
  const enabled = usePetsEnabled();
  if (enabled === null) {
    return (
      <>
        <PageHeader title="Pets" />
        <Loading>Loading…</Loading>
      </>
    );
  }
  return enabled ? <PetsView /> : <NotFound />;
}

/**
 * Pets: the photos with a dog or a cat as a timeline (the library's own, with selection, bulk actions and the viewer), a switch between
 * All, Dogs and Cats, and, while the worker is still looking through the library, how far it has got.
 */
function PetsView() {
  const { setAnonymous } = useAuth();
  const { toast, show } = useToast();
  const [params, setParams] = useSearchParams();
  const [summary, setSummary] = useState<Summary>({ status: "loading" });
  const [saving, setSaving] = useState<{ target: Target; count: number } | null>(null);
  const [named, setNamed] = useState<NamedPet[] | null>(null);
  // "Name a pet": selection mode is asked for (the signal goes up); the photo grid then names the picked photos.
  const [nameSignal, setNameSignal] = useState(0);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  const load = useCallback(async () => {
    try {
      const data = await api.pets();
      setSummary({ status: "ready", data });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setSummary((cur) => (cur.status === "ready" ? cur : { status: "error", message: e instanceof Error ? e.message : "Could not load Pets." }));
    }
  }, [setAnonymous]);
  useEffect(() => {
    void load();
  }, [load]);
  // The pets the owner named, with how many proposals wait for each; a failure leaves the row out.
  const loadNamed = useCallback(
    () =>
      api.namedPets().then(
        (r) => setNamed(r.items),
        () => setNamed((cur) => cur ?? []),
      ),
    [],
  );
  useEffect(() => {
    void loadNamed();
  }, [loadNamed]);
  // Live: the worker finds pets in the background; read the counts again now and then while the page is open.
  useEffect(() => {
    const t = setInterval(() => {
      if (document.visibilityState === "hidden") return;
      api.pets().then(
        (p) => setSummary((cur) => (cur.status === "ready" && sameCounts(cur.data, p) ? cur : { status: "ready", data: p })),
        () => undefined,
      );
      void loadNamed();
    }, LIVE_EVERY_MS);
    return () => clearInterval(t);
  }, [loadNamed]);

  const data = summary.status === "ready" ? summary.data : null;
  const asked = petKindOf(params.get("kind")) ?? "any";
  const kinds = data ? offeredKinds(data) : (["any"] as PetKind[]);
  // A kind with no photos is not offered: a link to it shows All.
  const kind: PetKind = kinds.includes(asked) ? asked : "any";
  const photos = data ? petCount(data, kind) : 0;

  const askToName = () => {
    setNameSignal((n) => n + 1);
    window.scrollTo({ top: 0 });
  };

  const header = (
    <PageHeader
      title="Pets"
      actions={
        data && data.any > 0 ? (
          <Menu
            label="Pets"
            trigger={
              <Button size="icon" variant="ghost" aria-label="More">
                <MoreHorizontal />
              </Button>
            }
          >
            <MenuItem onSelect={askToName}>
              <Tag />
              Name a pet
            </MenuItem>
            <MenuItem onSelect={() => setSaving({ target: { scope: { pets: kind }, except: [] }, count: photos })}>
              <FolderPlus />
              Save as album
            </MenuItem>
            <MenuItem asChild>
              <Link to={`/search?pets=${kind}`}>
                <Search />
                Search with filters
              </Link>
            </MenuItem>
          </Menu>
        ) : undefined
      }
    >
      {/* With one kind, All is that kind: nothing to switch between. */}
      {kinds.length > 2 && (
        <Segmented
          label="Show"
          value={kind}
          onChange={(k) => setParams(k === "any" ? {} : { kind: k }, { replace: true })}
          options={kinds.map((k) => ({ value: k, label: PET_TAB[k] }))}
        />
      )}
    </PageHeader>
  );

  let body: ReactNode;
  if (summary.status === "loading") {
    body = <Loading>Loading…</Loading>;
  } else if (summary.status === "error" || !data) {
    body = (
      <Empty
        icon={TriangleAlert}
        title="Couldn’t load Pets"
        action={
          <Button variant="primary" onClick={() => void load()}>
            Try again
          </Button>
        }
      >
        {summary.status === "error" ? summary.message : ""}
      </Empty>
    );
  } else if (data.any === 0) {
    body = <NoPets summary={data} />;
  } else {
    body = (
      <>
        {scanning(data) && <Progress summary={data} />}
        <YourPets pets={named} onName={askToName} />
        <PetsPhotos
          key={kind}
          kind={kind}
          photos={photos}
          failure={failure}
          show={show}
          onChanged={async () => {
            await Promise.all([load(), loadNamed()]);
          }}
          onSave={(target, count) => setSaving({ target, count })}
          nameSignal={nameSignal}
        />
      </>
    );
  }

  // (One header for every state, so it stays put while the answer arrives.)
  return (
    <>
      {header}
      {body}
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
      {toast}
    </>
  );
}

const sameCounts = (a: PetsSummary, b: PetsSummary) =>
  a.any === b.any && a.scanned === b.scanned && a.total === b.total && a.kinds.every((k, i) => k.photos === b.kinds[i]?.photos && k.kind === b.kinds[i]?.kind);

/**
 * The pets the owner named, as round covers with their names (a count on the ones with "Is this X?" proposals waiting), above the photos; and
 * the way to name one. Without any, a line says how.
 */
function YourPets({ pets, onName }: { pets: NamedPet[] | null; onName: () => void }) {
  const id = useId();
  if (pets === null) return null;
  return (
    <section aria-labelledby={id} className="mx-auto max-w-[96rem] pt-3">
      <div className="mb-3 flex items-center justify-between gap-4 px-4 md:px-8">
        <h2 id={id} className="text-[1.0625rem] font-semibold tracking-tight">
          Your pets
        </h2>
        <Button variant="secondary" size="sm" onClick={onName}>
          <Plus />
          Name a pet
        </Button>
      </div>
      {pets.length > 0 ? (
        <ScrollRow label="Your pets" wrap>
          {[...pets.filter((p) => !p.hidden), ...pets.filter((p) => p.hidden)].map((p) => (
            <NamedPetCard key={p.id} pet={p} />
          ))}
        </ScrollRow>
      ) : (
        <p className="px-4 text-[0.9375rem] text-muted md:px-8">Pick a few photos of your dog or cat and give them a name. The app then suggests more photos of them.</p>
      )}
    </section>
  );
}

/** The bar and the line of the backfill: how many of the photos have been looked at. */
function ProgressBar({ summary }: { summary: PetsSummary }) {
  const percent = scanPercent(summary);
  return (
    <div
      className="h-1 overflow-hidden rounded-full bg-line"
      role="progressbar"
      aria-label="Looking for pets"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent ?? undefined}
    >
      <span className="block h-full rounded-full bg-accent transition-[width] duration-500 ease-soft" style={{ width: `${percent ?? 0}%` }} />
    </div>
  );
}

/** Above the photos while the rest of the library is still being looked through. */
function Progress({ summary }: { summary: PetsSummary }) {
  return (
    <section aria-label="Looking for pets" className="mx-4 mt-3 rounded-card bg-surface px-4 py-2.5 md:mx-8">
      <p className="mb-2 truncate text-sm" role="status">
        {scanLine(summary)}. More may turn up.
      </p>
      <ProgressBar summary={summary} />
    </section>
  );
}

/** No photo has a pet (yet): either the worker is still looking, or it found none. */
function NoPets({ summary }: { summary: PetsSummary }) {
  if (scanning(summary)) {
    return (
      <Enter className="mx-auto flex max-w-sm flex-col items-center gap-2 px-4 py-20 text-center">
        <span className="mb-2 flex size-14 items-center justify-center rounded-full bg-surface text-muted">
          <PawPrint className="size-6" strokeWidth={1.6} aria-hidden="true" />
        </span>
        <h2 className="text-[1.0625rem] font-semibold tracking-tight">Looking for dogs and cats</h2>
        <p className="text-[0.9375rem] text-muted">Your library is being looked through. Pets show up here as they are found.</p>
        <div className="mt-3 w-full">
          <ProgressBar summary={summary} />
          <p className="mt-2 text-sm text-muted" role="status">
            {scanLine(summary)}
          </p>
        </div>
      </Enter>
    );
  }
  return (
    <Empty icon={PawPrint} title="No pets found">
      Photos with a dog or a cat appear here once they are in your library.
    </Empty>
  );
}

function PetsPhotos({
  kind,
  photos,
  failure,
  show,
  onChanged,
  onSave,
  nameSignal,
}: {
  kind: PetKind;
  /** How many photos the kind has now, as the header counts say. */
  photos: number;
  failure: (e: unknown, what: string) => void;
  show: (t: { message: string; tone?: "error"; action?: { label: string; onClick: () => void } }) => void;
  onChanged: () => Promise<void>;
  /** "Save as album": the photos to save (explicit ids, or the whole view) and how many that is. */
  onSave: (target: Target, count: number) => void;
  /** "Name a pet" asked for selection mode (a number that goes up). */
  nameSignal: number;
}) {
  const view = useMemo(() => ({ pets: kind }), [kind]);
  const scope = useMemo<Scope>(() => ({ pets: kind }), [kind]);
  return (
    <ScopePhotos
      view={view}
      scope={scope}
      changed={String(photos)}
      words={petPhotos}
      slideshowTitle={PET_LABEL[kind]}
      empty={`Photos with ${kind === "dog" ? "a dog" : kind === "cat" ? "a cat" : "a dog or a cat"} appear here.`}
      hiddenWhere="Hidden"
      selectSignal={nameSignal}
      signalLabel="Select photos of your pet"
      failure={failure}
      show={show}
      onChanged={onChanged}
      onSave={onSave}
    />
  );
}
