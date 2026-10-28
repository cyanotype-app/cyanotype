import { Link } from "react-router-dom";
import type { Album, NamedPet, Person, Trip } from "../api";
import { FaceCrop } from "../components/FaceCrop";
import { badgeText, isNamed, openPersonLabel, pendingLabel, personLabel } from "../peopleText";
import { namedPetLabel, petPhotos } from "../petsText";
import { cn } from "../ui/cn";
import { Cover } from "./Cover";
import { albumCount, tripLine, tripName } from "./text";

const focus = "rounded-card outline-offset-2 transition-transform duration-150 ease-soft active:scale-[0.985]";

/** A large card of the Memories row: the photo fills it and the title sits on a soft shade at the bottom. */
export function MemoryCard({
  to,
  title,
  subtitle,
  name,
  cover,
  chip,
}: {
  to: string;
  title: string;
  subtitle: string;
  /** Accessible name; defaults to the visible text. */
  name?: string;
  cover: { thumb_url: string | null; thumbhash: string | null } | null;
  chip?: string;
}) {
  return (
    <li className="shrink-0 snap-start">
      <Link
        to={to}
        aria-label={name}
        className={cn(
          focus,
          "group relative block aspect-[4/5] w-[min(17rem,72vw)] overflow-hidden bg-surface text-white no-underline md:aspect-[4/3] md:w-[19rem]",
        )}
      >
        <Cover
          src={cover?.thumb_url}
          thumbhash={cover?.thumbhash}
          className="absolute inset-0"
          imgClassName="transition-transform duration-500 ease-soft group-hover:scale-[1.03]"
        />
        <span className="absolute inset-x-0 bottom-0 flex flex-col gap-0.5 bg-gradient-to-t from-black/65 via-black/25 to-transparent px-4 pt-14 pb-4">
          <strong className="truncate text-lg leading-tight font-semibold tracking-tight">{title}</strong>
          <span className="truncate text-sm text-white/85">{subtitle}</span>
        </span>
        {chip && (
          <span className="absolute top-3 left-3 rounded-full bg-black/40 px-2.5 py-1 text-xs font-medium backdrop-blur-md">{chip}</span>
        )}
      </Link>
    </li>
  );
}

/** A trip of the "Recent places" row: its cover, the place, and when. */
export function TripCard({ trip }: { trip: Trip }) {
  return (
    <li className="shrink-0 snap-start">
      <Link
        to={`/trips/${encodeURIComponent(trip.id)}`}
        state={{ trip }}
        aria-label={tripName(trip)}
        className={cn(focus, "group block w-60 no-underline md:w-72")}
      >
        <Cover
          src={trip.cover.thumb_url}
          thumbhash={trip.cover.thumbhash}
          className="aspect-[4/3] rounded-card"
          imgClassName="transition-transform duration-500 ease-soft group-hover:scale-[1.03]"
        />
        <span className="mt-2 block truncate text-[0.9375rem] font-medium text-fg">{trip.title}</span>
        <span className="block truncate text-[0.8125rem] text-muted">{tripLine(trip)}</span>
      </Link>
    </li>
  );
}

/** An album of the Albums row: a square cover, the name, how many photos. */
export function AlbumCard({ album }: { album: Album }) {
  return (
    <li className="shrink-0 snap-start">
      <Link
        to={`/albums/${encodeURIComponent(album.id)}`}
        aria-label={`${album.title}, ${albumCount(album)}`}
        className={cn(focus, "group block w-36 no-underline md:w-44")}
      >
        <Cover
          src={album.cover?.thumb_url}
          thumbhash={album.cover?.thumbhash}
          className="aspect-square rounded-card"
          imgClassName="transition-transform duration-500 ease-soft group-hover:scale-[1.03]"
        />
        <span className="mt-2 block truncate text-[0.9375rem] font-medium text-fg">{album.title}</span>
        <span className="block truncate text-[0.8125rem] text-muted">{albumCount(album)}</span>
      </Link>
    </li>
  );
}

/**
 * A pet the owner named: the round crop of its cover, the name and how many photos; "Is this X?" proposals waiting wear a small count in the
 * corner, like a person's card. The whole card opens the pet's page.
 */
export function NamedPetCard({ pet }: { pet: NamedPet }) {
  return (
    <li className="shrink-0 snap-start">
      <Link
        to={`/pets/${encodeURIComponent(pet.id)}`}
        aria-label={namedPetLabel(pet)}
        className="group flex w-24 flex-col items-center gap-1 rounded-card no-underline outline-offset-2 md:w-28"
      >
        <span className="relative mb-0.5 block size-[4.5rem] md:size-[5.5rem]">
          <span className="block size-full overflow-hidden rounded-full bg-surface transition-transform duration-150 ease-soft group-active:scale-95">
            {pet.cover && <FaceCrop url={pet.cover.preview_url} box={pet.cover.box} />}
          </span>
          {pet.pending > 0 && (
            <span
              data-badge=""
              className="absolute -top-0.5 -right-0.5 flex min-w-5 items-center justify-center rounded-full bg-accent px-1.5 text-[0.6875rem] leading-5 font-semibold text-on-accent ring-2 ring-bg"
              aria-hidden="true"
              title={pendingLabel(pet.pending)}
            >
              {badgeText(pet.pending)}
            </span>
          )}
        </span>
        <span className="w-full truncate text-center text-[0.8125rem] font-medium text-fg">{pet.name}</span>
        <span className="w-full truncate text-center text-xs text-muted">{pet.hidden ? "Hidden" : petPhotos(pet.photos)}</span>
      </Link>
    </li>
  );
}

/** A person of the People row: a round face and the name. */
export function PersonCard({ person }: { person: Person }) {
  return (
    <li className="shrink-0 snap-start">
      <Link
        to={`/people/${encodeURIComponent(person.id)}`}
        aria-label={openPersonLabel(person)}
        className="group flex w-20 flex-col items-center gap-1.5 rounded-card no-underline outline-offset-2 md:w-24"
      >
        <span className="block size-[4.5rem] overflow-hidden rounded-full bg-surface transition-transform duration-150 ease-soft group-active:scale-95 md:size-[5.5rem]">
          {person.cover && <FaceCrop url={person.cover.preview_url} box={person.cover.box} />}
        </span>
        <span className={cn("w-full truncate text-center text-[0.8125rem]", isNamed(person) ? "text-fg" : "text-muted")}>
          {isNamed(person) ? personLabel(person) : "Unnamed"}
        </span>
      </Link>
    </li>
  );
}
