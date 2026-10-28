import { lazy, Suspense, useEffect, useState, type MouseEvent, type ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, type PhotoDetails, type PhotoFace, type PhotoPet, type PhotoTag, type TilesInstalled } from "../api";
import { burstText, coordinatesText, dayHref, mapHref, resolutionText, searchHref, whenText } from "../detailsText";
import { placeLine } from "../map/geo";
import { fileSize } from "../pairText";
import { peopleNamed } from "../peopleText";
import { KIND_TITLE } from "../petsText";
import { PetKindMenu } from "./PetKindMenu";
import { fromServer, recipeText } from "../recipeText";
import { tagSearchHref } from "../tagText";
import { Button } from "../ui/Button";
import { Switch } from "../ui/Controls";
import { cn } from "../ui/cn";
import type { EditKind } from "./EditDialog";
import { FaceCrop } from "./FaceCrop";
import { RatingStars } from "./Stars";
import { TagChip } from "./TagChip";
import { ViewerPanel } from "./ViewerPanel";

// maplibre-gl is big: the small map loads only when a photo with a location is looked at.
const PlaceMap = lazy(() => import("../map/PlaceMap"));

const darkQuery = () => window.matchMedia?.("(prefers-color-scheme: dark)");

export type DetailsState =
  | { status: "loading" }
  | { status: "error" }
  | { status: "ready"; details: PhotoDetails };

interface Props {
  /** The id the Info button points at (`aria-controls`). */
  panelId: string;
  state: DetailsState;
  onRetry: () => void;
  onClose: () => void;
  /** Take the panel's history entry off before a link is followed, so Back from the next page lands on this one. */
  onLeave: () => Promise<void>;
  /** A link inside the panel was followed: the lightbox closes with it. */
  onNavigate: () => void;
  /** Whether the panel is shown (the viewer keeps it mounted while it animates away). */
  open?: boolean;
  /** Open the editor for the photo's date, location or caption. Without it the panel only shows. */
  onEdit?: (kind: EditKind) => void;
  /** The photo's stars, and how to change them (a row of stars shows when `onRate` is given). */
  rating?: number;
  onRate?: (rating: number) => void;
  /** The photo's tags, and how to change them (the row shows when `onTag` is given). */
  tags?: PhotoTag[];
  onTag?: () => void;
  onUntag?: (tag: PhotoTag) => void;
  /** Open the rotate/crop editor. The panel then has a Picture row describing the recipe, with the button. */
  onEditPicture?: () => void;
  /** The faces found in the photo (People is on): a row of chips for the named people, each opening its person. */
  faces?: PhotoFace[];
  /** How many detections the owner marked "Not a face" in this photo, and the way to bring them back. */
  dismissed?: number;
  onBringBack?: () => void;
  /** Whether the faces are outlined on the photo, and the switch for it (offered when the photo has faces to click). */
  faceBoxes?: boolean;
  onFaceBoxes?: (on: boolean) => void;
  /** The animals found in the photo (Pets is on): each with the pet it is in, or a way to put it in one. */
  pets?: PetsPanel;
}

/** What the panel's Pets row needs: the animals, the picture to crop them from, and what can be done with one. */
export interface PetsPanel {
  animals: PhotoPet[];
  /** The photo's picture (its preview), cropped to each animal; without it the animals are listed without a crop. */
  picture: string | null;
  /** Put the (unnamed) animal in a pet. */
  onAdd: (animal: PhotoPet) => void;
  /** "Not this pet": take the animal out of the pet it is in. */
  onNot: (animal: PhotoPet) => void;
  /** "It's a dog" / "It's a cat": the owner's word on what the animal is. */
  onKind: (animal: PhotoPet, kind: "dog" | "cat") => void;
}

type Follow = (e: MouseEvent<HTMLAnchorElement>, to: string) => void;

/** A link inside the panel: a plain click first closes the panel's history entry, then the lightbox, then navigates. */
function PanelLink({ to, follow, className, children, ...rest }: { to: string; follow: Follow; className?: string; children: ReactNode; "aria-label"?: string }) {
  return (
    <Link to={to} className={cn("text-accent hover:underline", className)} onClick={(e) => follow(e, to)} {...rest}>
      {children}
    </Link>
  );
}

/** A group of rows on a rounded well, hairlines between them. */
function Group({ children, label }: { children: ReactNode; label?: string }) {
  return (
    <dl aria-label={label} className="divide-y divide-line overflow-hidden rounded-card bg-surface">
      {children}
    </dl>
  );
}

function Row({ label, children, action }: { label: string; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex items-start gap-3 px-4 py-3">
      <div className="min-w-0 flex-1">
        <dt className="text-[0.8125rem] text-muted">{label}</dt>
        <dd className="mt-0.5 flex min-w-0 flex-col gap-0.5 text-[0.9375rem] [overflow-wrap:anywhere]">{children}</dd>
      </div>
      {action}
    </div>
  );
}

const sub = "text-sm text-muted";

function EditButton({ label, onClick }: { label: string; onClick: () => void }) {
  const text = label.startsWith("Add") || label.startsWith("Edit") ? label.split(" ")[0] : "Adjust";
  return (
    <Button variant="ghost" size="sm" className="-mr-2 shrink-0 self-start text-accent" aria-label={label} onClick={onClick}>
      {text}
    </Button>
  );
}

/**
 * The details of the open photo, Apple Photos style: when, where (with a small map), camera and lens, the exposure, the
 * files, albums and burst, in grouped rows. A row appears only when the photo has the information. The camera, lens,
 * place and date are links to the search or the timeline for the same thing. A bottom sheet on a phone, a column beside the
 * photo on a desktop.
 */
export function DetailsPanel({ panelId, state, onRetry, onClose, onLeave, onNavigate, open = true, onEdit, rating, onRate, tags, onTag, onUntag, onEditPicture, faces, dismissed, onBringBack, faceBoxes, onFaceBoxes, pets }: Props) {
  const navigate = useNavigate();
  const follow: Follow = (e, to) => {
    // Let the browser handle new-tab clicks and the like.
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    void onLeave().then(() => {
      onNavigate();
      navigate(to);
    });
  };

  return (
    <ViewerPanel id={panelId} title="Info" label="Photo details" closeLabel="Close info" onClose={onClose} open={open}>
      {state.status === "loading" && (
        <p className="py-4 text-muted" role="status">
          Loading details…
        </p>
      )}
      {state.status === "error" && (
        <div role="alert" className="flex flex-col items-start gap-3 py-4">
          <p>Could not load the details.</p>
          <Button variant="secondary" onClick={onRetry}>
            Try again
          </Button>
        </div>
      )}
      {state.status === "ready" && (
        <Body d={state.details} follow={follow} onEdit={onEdit} rating={rating} onRate={onRate} tags={tags} onTag={onTag} onUntag={onUntag} onEditPicture={onEditPicture} faces={faces} dismissed={dismissed} onBringBack={onBringBack} faceBoxes={faceBoxes} onFaceBoxes={onFaceBoxes} pets={pets} />
      )}
    </ViewerPanel>
  );
}

function Body({
  d,
  follow,
  onEdit,
  rating,
  onRate,
  tags,
  onTag,
  onUntag,
  onEditPicture,
  faces,
  dismissed,
  onBringBack,
  faceBoxes,
  onFaceBoxes,
  pets,
}: {
  d: PhotoDetails;
  follow: Follow;
  onEdit?: (kind: EditKind) => void;
  rating?: number;
  onRate?: (rating: number) => void;
  tags?: PhotoTag[];
  onTag?: () => void;
  onUntag?: (tag: PhotoTag) => void;
  onEditPicture?: () => void;
  faces?: PhotoFace[];
  dismissed?: number;
  onBringBack?: () => void;
  faceBoxes?: boolean;
  onFaceBoxes?: (on: boolean) => void;
  pets?: PetsPanel;
}) {
  const when = whenText(d.date);
  const loc = d.location;
  const place = loc?.place ? placeLine(loc.place) : "";
  const resolution = resolutionText(d.file);
  const x = d.exposure;
  const pair = d.files.length > 1;
  const named = peopleNamed(faces);
  const hasCamera = !!(d.camera?.label || d.camera?.lens || x?.line || x?.focal_length_35mm || x?.flash || x?.white_balance || x?.metering || x?.program);
  const hasPets = (pets?.animals.length ?? 0) > 0;
  const hasPeople = named.length > 0 || (onFaceBoxes && (faces?.length ?? 0) > 0) || (onBringBack && (dismissed ?? 0) > 0);

  return (
    <div className="flex flex-col gap-4">
      {onRate && (
        <div className="flex items-center justify-between gap-3 rounded-card bg-surface px-4 py-1.5">
          <span className="text-[0.8125rem] text-muted">Rating</span>
          <RatingStars value={rating} onChange={onRate} label="Rating of this photo" />
        </div>
      )}

      <Group>
        <Row label={d.date.basis === "taken" ? "Date" : d.date.basis === "file" ? "File date" : "Added"} action={onEdit && <EditButton label="Adjust date and time" onClick={() => onEdit("date")} />}>
          <PanelLink to={dayHref(d.date.timeline_day)} follow={follow}>
            {when.day}
          </PanelLink>
          <span className={sub}>
            {when.time}
            {when.time && " · "}
            <span className={d.date.zone_known ? undefined : "italic"}>{when.zone}</span>
          </span>
          {when.note && <span className={sub}>{when.note}</span>}
          {d.edits?.date_edited && <span className={cn(sub, "italic")}>Edited</span>}
        </Row>

        {loc && (
          <Row label="Location" action={onEdit && <EditButton label="Edit location" onClick={() => onEdit("location")} />}>
            {loc.place && (
              <PanelLink to={searchHref({ place: loc.place.city })} follow={follow}>
                {place}
              </PanelLink>
            )}
            <span className={sub}>{coordinatesText(loc.latitude, loc.longitude)}</span>
            {loc.altitude && <span className={sub}>Altitude {loc.altitude}</span>}
            {loc.direction && <span className={sub}>Facing {loc.direction}</span>}
            {d.edits?.location_edited && <span className={cn(sub, "italic")}>Edited</span>}
            <MiniMap lat={loc.latitude} lon={loc.longitude} follow={follow} />
          </Row>
        )}
        {!loc && onEdit && (
          <Row label="Location" action={<EditButton label="Add location" onClick={() => onEdit("location")} />}>
            <span className={sub}>No location</span>
          </Row>
        )}

        {(d.caption || onEdit) && (
          <Row label="Caption" action={onEdit && <EditButton label={d.caption ? "Edit caption" : "Add a caption"} onClick={() => onEdit("caption")} />}>
            {d.caption ? <span className="whitespace-pre-wrap">{d.caption}</span> : <span className={sub}>No caption</span>}
          </Row>
        )}
      </Group>

      {(onTag || hasPeople || hasPets || d.albums.length > 0) && (
        <Group>
          {onTag && (
            <Row label="Tags">
              <span className="mt-1 flex flex-wrap items-center gap-1.5">
                {tags?.map((t) => (
                  <TagChip key={t.id} className="bg-card" imported={t.imported} title={t.imported ? "From the photo's own keywords" : undefined} removeLabel={`Remove tag ${t.name}`} onRemove={onUntag && (() => onUntag(t))}>
                    <PanelLink to={tagSearchHref(t.name)} follow={follow} className="text-fg">
                      {t.name}
                    </PanelLink>
                  </TagChip>
                ))}
                <Button variant="secondary" size="sm" className="bg-card hover:bg-card/70" onClick={onTag}>
                  Add tag
                </Button>
              </span>
            </Row>
          )}

          {hasPeople && (
            <Row label="People">
              {named.length > 0 && (
                <span className="mt-1 flex flex-wrap gap-1.5">
                  {named.map((p) => (
                    <TagChip key={p.id} className="bg-card">
                      <PanelLink to={`/people/${p.id}`} follow={follow} className="text-fg">
                        {p.name}
                      </PanelLink>
                    </TagChip>
                  ))}
                </span>
              )}
              {onFaceBoxes && (faces?.length ?? 0) > 0 && <Switch checked={faceBoxes === true} onChange={onFaceBoxes} label="Show face boxes on the photo" />}
              {onBringBack && (dismissed ?? 0) > 0 && (
                <Button variant="secondary" size="sm" className="mt-1 h-auto self-start bg-card py-2 whitespace-normal text-left hover:bg-card/70" onClick={onBringBack}>
                  Bring back {dismissed === 1 ? "1 detection" : `${dismissed} detections`} marked “Not a face”
                </Button>
              )}
            </Row>
          )}

          {pets && hasPets && (
            <Row label="Pets">
              <ul className="mt-1 flex flex-col gap-2">
                {pets.animals.map((a, i) => {
                  const many = pets.animals.length > 1;
                  return (
                    <li key={a.id} className="flex items-center gap-3">
                      {pets.picture && <FaceCrop url={pets.picture} box={a.box} className="size-10 shrink-0" />}
                      <span className="min-w-0 flex-1">
                        {a.pet ? (
                          <PanelLink to={`/pets/${a.pet.id}`} follow={follow} className="text-fg">
                            {a.pet.name}
                          </PanelLink>
                        ) : (
                          <span>
                            {KIND_TITLE[a.kind]}
                            {many ? ` ${i + 1}` : ""} <span className={sub}>not named</span>
                          </span>
                        )}
                      </span>
                      {a.pet ? (
                        <Button
                          variant="secondary"
                          size="sm"
                          className="shrink-0 bg-card hover:bg-card/70"
                          aria-label={many ? `Not ${a.pet.name}, ${a.kind} ${i + 1}` : undefined}
                          onClick={() => pets.onNot(a)}
                        >
                          Not {a.pet.name}
                        </Button>
                      ) : (
                        <Button
                          variant="secondary"
                          size="sm"
                          className="shrink-0 bg-card hover:bg-card/70"
                          aria-label={many ? `Add to pet…, ${a.kind} ${i + 1}` : undefined}
                          onClick={() => pets.onAdd(a)}
                        >
                          Add to pet…
                        </Button>
                      )}
                      <PetKindMenu animal={a} label={`${KIND_TITLE[a.kind]}${many ? ` ${i + 1}` : ""}`} onKind={pets.onKind} />
                    </li>
                  );
                })}
              </ul>
            </Row>
          )}

          {d.albums.length > 0 && (
            <Row label="Albums">
              {d.albums.map((a) => (
                <PanelLink key={a.id} to={`/albums/${a.id}`} follow={follow}>
                  {a.title}
                </PanelLink>
              ))}
            </Row>
          )}
        </Group>
      )}

      {hasCamera && (
        <Group>
          {d.camera?.label && (
            <Row label="Camera">
              <PanelLink to={searchHref({ camera: d.camera.label })} follow={follow}>
                {d.camera.label}
              </PanelLink>
            </Row>
          )}
          {d.camera?.lens && (
            <Row label="Lens">
              <PanelLink to={searchHref({ lens: d.camera.lens })} follow={follow}>
                {d.camera.lens}
              </PanelLink>
            </Row>
          )}
          {x?.line && <Row label="Exposure">{x.line}</Row>}
          {x?.focal_length_35mm && <Row label="35 mm equivalent">{x.focal_length_35mm}</Row>}
          {x?.flash && <Row label="Flash">{x.flash}</Row>}
          {x?.white_balance && <Row label="White balance">{x.white_balance}</Row>}
          {x?.metering && <Row label="Metering">{x.metering}</Row>}
          {x?.program && <Row label="Exposure program">{x.program}</Row>}
        </Group>
      )}

      <Group>
        <Row label={pair ? "Files" : "File"}>
          {d.files.map((f) => (
            <span key={f.id} className="flex flex-col">
              <span>{f.name}</span>
              <span className={sub}>
                {f.format}
                {f.raw ? " (RAW)" : ""} · {fileSize(f.size_bytes)}
              </span>
            </span>
          ))}
        </Row>
        {resolution && <Row label="Resolution">{resolution}</Row>}
        {d.file.color_space && <Row label="Colour space">{d.file.color_space}</Row>}
        {d.file.software && <Row label="Software">{d.file.software}</Row>}
        {onEditPicture && (
          <Row label="Picture" action={<EditButton label="Rotate and crop the picture" onClick={onEditPicture} />}>
            <span className={sub}>{recipeText(fromServer(d.edits?.recipe), d.file.width, d.file.height)}</span>
            {d.edits?.recipe && <span className={cn(sub, "italic")}>Edited</span>}
          </Row>
        )}
        {d.burst && <Row label="Burst">{burstText(d.burst)}</Row>}
      </Group>
    </div>
  );
}

/** The map with a pin; a tap opens the map page there. Without WebGL only the coordinates (above) remain. */
function MiniMap({ lat, lon, follow }: { lat: number; lon: number; follow: Follow }) {
  const [tiles, setTiles] = useState<TilesInstalled | null>(null);
  const [failed, setFailed] = useState(false);
  const [dark, setDark] = useState(() => darkQuery()?.matches ?? false);
  useEffect(() => {
    let live = true;
    api
      .tiles()
      .then((t) => live && setTiles(t))
      .catch(() => live && setTiles({ world: false, detail: false })); // no basemap: the plain background with the pin
    return () => {
      live = false;
    };
  }, []);
  useEffect(() => {
    const q = darkQuery();
    if (!q) return;
    const on = () => setDark(q.matches);
    q.addEventListener("change", on);
    return () => q.removeEventListener("change", on);
  }, []);
  if (failed) return null;
  return (
    <Link
      to={mapHref(lat, lon)}
      onClick={(e) => follow(e, mapHref(lat, lon))}
      aria-label="Open the map at this photo"
      className="mt-2 block overflow-hidden rounded-[0.7rem] border border-line"
    >
      <Suspense fallback={<div className="place-map" />}>
        {tiles ? <PlaceMap lat={lat} lon={lon} basemap={tiles} dark={dark} onUnavailable={() => setFailed(true)} /> : <div className="place-map" />}
      </Suspense>
    </Link>
  );
}
