import { ChevronRight, ImagePlus, Link2 } from "lucide-react";
import { Link } from "react-router-dom";
import { useMemo } from "react";
import { api, localToday, type Trip } from "../api";
import { AlbumCard, MemoryCard, NamedPetCard, PersonCard, PetCard, TripCard } from "../collections/cards";
import { Cover } from "../collections/Cover";
import { ScrollRow } from "../collections/ScrollRow";
import { Reveal, RowSkeleton, Section } from "../collections/Section";
import { homePeople, isFreshTrip } from "../collections/text";
import { useLoad } from "../collections/useLoad";
import { LIBRARY, UTILITIES, type NavItem } from "../shell/nav";
import { UploadMenu } from "../shell/AppShell";
import { PageHeader } from "../shell/PageHeader";
import { useUploadPicker } from "../shell/UploadPicker";
import { shareApi, type ShareLink } from "../shareApi";
import { shareName } from "../shareText";
import { photoCount } from "../albumText";
import { Button } from "../ui/Button";
import { usePeopleEnabled } from "../usePeople";
import { usePetsEnabled } from "../usePets";
import { hasMemories, memoryCount, yearsAgo } from "../viewText";

/** Library views with no section of their own on this page (or whose section can be empty): reachable from the list at the bottom. */
const MORE = ["/favorites", "/tags", "/map", "/memories", "/shared"];

/**
 * The home page: the library as a set of collections. Each section loads on its own, so the page fills in from the top as
 * answers arrive; one with nothing to show is not drawn. A library with no photos and no albums gets an invitation to
 * upload instead. On a phone the list of remaining views at the bottom is the way into them (the sidebar has them on a
 * desktop, where the list would only repeat it).
 */
export function Collections() {
  const people = usePeopleEnabled();
  const petsOn = usePetsEnabled();
  const memories = useLoad(() => api.memories());
  const trips = useLoad(() => api.trips({ limit: 12 }));
  const albums = useLoad(() => api.listAlbums());
  const recent = useLoad(() => api.listAssets({ sort: "added", limit: 14 }));
  const faces = useLoad(() => api.listPeople(), people === true);
  const shares = useLoad(() => shareApi.list());
  const pets = useLoad(() => api.pets(), petsOn === true);
  const named = useLoad(() => api.namedPets(), petsOn === true);

  const today = useMemo(() => localToday(), []);
  const fresh = useMemo(
    () => (trips.status === "ready" && trips.data.enabled ? trips.data.trips.filter((t) => isFreshTrip(t, today)).slice(0, 2) : []),
    [trips, today],
  );

  const liveShares = shares.status === "ready" ? shares.data.items.filter((s) => !s.expired) : [];
  const showPeople = people === true && faces.status !== "failed";
  const two = showPeople && liveShares.length > 0;

  const empty =
    recent.status === "ready" && recent.data.items.length === 0 && albums.status === "ready" && albums.data.items.length === 0;

  return (
    <>
      <PageHeader title="Collections" actions={<span className="md:hidden"><UploadMenu compact /></span>} />
      <div className="mx-auto flex max-w-[90rem] flex-col gap-9 pt-4 pb-12 md:gap-11 md:pt-6">
        {empty ? (
          <Welcome />
        ) : (
          <>
            <MemoriesSection memories={memories} fresh={fresh} trips={trips} />
            <AlbumsSection albums={albums} />
            <PlacesSection trips={trips} />
            {(showPeople || liveShares.length > 0) && (
              <div className={`flex flex-col gap-9 md:gap-11 ${two ? "lg:grid lg:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)] lg:gap-0" : ""}`}>
                {showPeople && <PeopleSection faces={faces} />}
                <SharedSection shares={liveShares} />
              </div>
            )}
            <PetsSection pets={pets} named={named} />
            <RecentSection recent={recent} />
          </>
        )}
        <MoreLists />
      </div>
    </>
  );
}

type Load<T> = ReturnType<typeof useLoad<T>>;

function MemoriesSection({
  memories,
  fresh,
  trips,
}: {
  memories: Load<Awaited<ReturnType<typeof api.memories>>>;
  fresh: Trip[];
  trips: Load<Awaited<ReturnType<typeof api.trips>>>;
}) {
  if (memories.status === "loading" || trips.status === "loading")
    return (
      <Section title="Memories">
        <RowSkeleton card="aspect-[4/5] w-[min(17rem,72vw)] rounded-card md:aspect-[4/3] md:w-[19rem]" count={3} />
      </Section>
    );
  const years = memories.status === "ready" && memories.data.enabled && hasMemories(memories.data.years) ? memories.data.years : [];
  if (years.length === 0 && fresh.length === 0) return null;
  return (
    <Section title="Memories" to={years.length > 0 ? "/memories" : undefined}>
      <Reveal>
        <ScrollRow label="Memories">
          {years.map((y) => (
            <MemoryCard
              key={y.year}
              to={`/memories#y${y.year}`}
              title={yearsAgo(y.years_ago)}
              subtitle={`On this day · ${memoryCount(y.count)}`}
              name={`${yearsAgo(y.years_ago)}, ${memoryCount(y.count)}`}
              cover={y.items[0] ?? null}
            />
          ))}
          {fresh.map((t) => (
            <MemoryCard
              key={t.id}
              to={`/trips/${encodeURIComponent(t.id)}`}
              title={t.title}
              subtitle={`${t.date_label} · ${memoryCount(t.photos)}`}
              name={`${t.title}, ${t.date_label}, ${memoryCount(t.photos)}`}
              cover={t.cover}
              chip={t.ongoing ? "Now" : undefined}
            />
          ))}
        </ScrollRow>
      </Reveal>
    </Section>
  );
}

function AlbumsSection({ albums }: { albums: Load<Awaited<ReturnType<typeof api.listAlbums>>> }) {
  if (albums.status === "failed") return null;
  if (albums.status === "loading")
    return (
      <Section title="Albums">
        <RowSkeleton card="h-[12.5rem] w-36 rounded-card md:h-[14rem] md:w-44" count={6} />
      </Section>
    );
  // Most recently changed first: the API lists them that way.
  const items = albums.data.items.slice(0, 14);
  return (
    <Section title="Albums" to="/albums">
      <Reveal>
        {items.length === 0 ? (
          <div className="mx-4 flex flex-col items-start gap-3 rounded-card bg-card px-5 py-5 ring-1 ring-line md:mx-8 md:max-w-xl">
            <p className="text-[0.9375rem] text-muted">Group photos into albums to find them in one place.</p>
            <Button asChild size="sm">
              <Link to="/albums">Make an album</Link>
            </Button>
          </div>
        ) : (
          <ScrollRow label="Albums">
            {items.map((a) => (
              <AlbumCard key={a.id} album={a} />
            ))}
          </ScrollRow>
        )}
      </Reveal>
    </Section>
  );
}

function PlacesSection({ trips }: { trips: Load<Awaited<ReturnType<typeof api.trips>>> }) {
  if (trips.status === "failed") return null;
  if (trips.status === "loading")
    return (
      <Section title="Recent places">
        <RowSkeleton card="h-[11rem] w-60 rounded-card md:h-[13.5rem] md:w-72" count={4} />
      </Section>
    );
  const list = trips.data.enabled ? trips.data.trips : [];
  if (list.length === 0) return null;
  return (
    <Section title="Recent places" to="/map" linkLabel="Map" linkName="Open the map of your places">
      <Reveal>
        <ScrollRow label="Recent places">
          {list.map((t) => (
            <TripCard key={t.id} trip={t} />
          ))}
        </ScrollRow>
      </Reveal>
    </Section>
  );
}

function PeopleSection({ faces }: { faces: Load<Awaited<ReturnType<typeof api.listPeople>>> }) {
  if (faces.status !== "ready")
    return (
      <Section title="People">
        <RowSkeleton card="size-20 rounded-full md:size-24" count={7} />
      </Section>
    );
  const shown = homePeople(faces.data.items);
  return (
    <Section title="People" to="/people">
      <Reveal>
        {shown.length === 0 ? (
          <p className="px-4 text-[0.9375rem] text-muted md:px-8">People you appear with show up here once faces are found in your photos.</p>
        ) : (
          <ScrollRow label="People" wrap>
            {shown.map((p) => (
              <PersonCard key={p.id} person={p} />
            ))}
          </ScrollRow>
        )}
      </Reveal>
    </Section>
  );
}

/** Dogs and cats, only while Pets is on and at least one photo has one (the load is not even asked while it is off). */
function PetsSection({ pets, named }: { pets: Load<Awaited<ReturnType<typeof api.pets>>>; named: Load<Awaited<ReturnType<typeof api.namedPets>>> }) {
  if (pets.status !== "ready" || pets.data.any === 0) return null;
  const shown = pets.data.kinds.filter((k) => k.photos > 0);
  // The pets the owner named come first, each its own card; the kinds follow.
  const mine = named.status === "ready" ? named.data.items.filter((p) => !p.hidden) : [];
  return (
    <Section title="Pets" to="/pets">
      <Reveal>
        <ScrollRow label="Pets">
          {mine.map((p) => (
            <NamedPetCard key={p.id} pet={p} />
          ))}
          {shown.map((k) => (
            <PetCard key={k.kind} kind={k.kind} photos={k.photos} cover={k.cover} />
          ))}
        </ScrollRow>
      </Reveal>
    </Section>
  );
}

function SharedSection({ shares: live }: { shares: ShareLink[] }) {
  if (live.length === 0) return null;
  return (
    <Section title="Shared links" to="/shared">
      <Reveal className="px-4 md:px-8">
        <ul className="divide-y divide-line overflow-hidden rounded-card bg-card ring-1 ring-line">
          {live.slice(0, 3).map((s) => (
            <li key={s.id}>
              <Link to="/shared" className="flex min-h-14 items-center gap-3 px-4 py-2 text-fg no-underline transition-colors hover:bg-surface">
                <Link2 className="size-5 shrink-0 text-muted" aria-hidden="true" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[0.9375rem]">{shareName(s)}</span>
                  <span className="block truncate text-[0.8125rem] text-muted">
                    {photoCount(s.item_count)} · {s.view_count} view{s.view_count === 1 ? "" : "s"}
                  </span>
                </span>
                <ChevronRight className="size-4 shrink-0 text-muted" aria-hidden="true" />
              </Link>
            </li>
          ))}
        </ul>
      </Reveal>
    </Section>
  );
}

function RecentSection({ recent }: { recent: Load<Awaited<ReturnType<typeof api.listAssets>>> }) {
  if (recent.status === "failed") return null;
  if (recent.status === "loading")
    return (
      <Section title="Recently added">
        <RowSkeleton card="size-28 rounded-tile md:size-36" count={8} />
      </Section>
    );
  const items = recent.data.items;
  if (items.length === 0) return null;
  return (
    <Section title="Recently added" to="/recent">
      <Reveal>
        <ScrollRow label="Recently added" className="[&_ul]:gap-1.5 md:[&_ul]:gap-2">
          {items.map((p) => (
            <li key={p.id} className="shrink-0 snap-start">
              <Link
                to="/recent"
                aria-label={`${p.original_name}, recently added`}
                className="block size-28 overflow-hidden rounded-tile outline-offset-2 transition-transform duration-150 ease-soft active:scale-95 md:size-36"
              >
                <Cover src={p.thumb_url} thumbhash={p.thumbhash} className="size-full" />
              </Link>
            </li>
          ))}
        </ScrollRow>
      </Reveal>
    </Section>
  );
}

/** A library with nothing in it yet: an invitation to put something there. */
function Welcome() {
  const { pick } = useUploadPicker();
  return (
    <Reveal className="mx-4 flex flex-col items-center gap-4 rounded-sheet bg-card px-6 py-14 text-center ring-1 ring-line md:mx-8 md:py-20">
      <span className="flex size-16 items-center justify-center rounded-full bg-accent/12 text-accent">
        <ImagePlus className="size-7" aria-hidden="true" />
      </span>
      <div className="max-w-sm">
        <h2 className="text-xl font-semibold tracking-tight">Start your library</h2>
        <p className="mt-1.5 text-[0.9375rem] text-muted">
          Upload photos and they appear here, gathered into albums, places and memories as your library grows.
        </p>
      </div>
      <div className="flex flex-wrap justify-center gap-2">
        <Button variant="primary" size="lg" onClick={() => pick("files")}>
          Upload photos
        </Button>
        <Button size="lg" onClick={() => pick("folder")}>
          Upload a folder
        </Button>
      </div>
    </Reveal>
  );
}

/**
 * The views that have no section above (or whose section can be missing), then housekeeping. Phones only: the tab bar has
 * no room for them, so this is the way in. A desktop has the sidebar, which lists the same, so repeating them there would
 * be a second menu on the page.
 */
function MoreLists() {
  const people = usePeopleEnabled() === true;
  const library = LIBRARY.filter((i) => MORE.includes(i.to) && (!i.people || people));
  return (
    <div className="flex flex-col gap-8 md:hidden">
      <LinkList title="Library" items={library} />
      <LinkList title="Utilities" items={UTILITIES} />
    </div>
  );
}

function LinkList({ title, items }: { title: string; items: readonly NavItem[] }) {
  return (
    <section aria-label={title} className="px-4">
      <h2 className="mb-2 text-[1.0625rem] font-semibold tracking-tight">{title}</h2>
      <ul className="divide-y divide-line overflow-hidden rounded-card bg-card ring-1 ring-line">
        {items.map((i) => {
          const Icon = i.icon;
          return (
            <li key={i.to}>
              <Link
                to={i.to}
                className="flex h-12 items-center gap-3 px-4 text-[0.9375rem] text-fg no-underline transition-colors hover:bg-surface active:bg-surface"
              >
                <Icon className="size-5 text-accent" aria-hidden="true" />
                <span className="flex-1">{i.label}</span>
                <ChevronRight className="size-4 text-muted" aria-hidden="true" />
              </Link>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
