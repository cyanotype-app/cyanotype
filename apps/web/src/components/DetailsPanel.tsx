import { lazy, Suspense, useEffect, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, type PhotoDetails, type TilesInstalled } from "../api";
import { burstText, coordinatesText, dayHref, mapHref, resolutionText, searchHref, whenText } from "../detailsText";
import { placeLine } from "../map/geo";
import { fileSize } from "../pairText";
import type { EditKind } from "./EditDialog";
import { useVerticalSwipe } from "../useInfoPanel";
import { RatingStars } from "./Stars";
import type { PhotoFace, PhotoTag } from "../api";
import { peopleNamed } from "../peopleText";
import { tagSearchHref } from "../tagText";
import { fromServer, recipeText } from "../recipeText";

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
}

type Follow = (e: MouseEvent<HTMLAnchorElement>, to: string) => void;

/** A link inside the panel: a plain click first closes the panel's history entry, then the lightbox, then navigates. */
function PanelLink({ to, follow, className, children, ...rest }: { to: string; follow: Follow; className?: string; children: ReactNode; "aria-label"?: string }) {
  return (
    <Link to={to} className={className} onClick={(e) => follow(e, to)} {...rest}>
      {children}
    </Link>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="details-row">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/**
 * The details of the open photo, Apple Photos style: when, where (with a small map), camera and lens, the exposure, the
 * files, albums and burst. A row appears only when the photo has the information. The camera, lens, place and date
 * are links to the search or the timeline for the same thing.
 */
export function DetailsPanel({ panelId, state, onRetry, onClose, onLeave, onNavigate, onEdit, rating, onRate, tags, onTag, onUntag, onEditPicture, faces, dismissed, onBringBack, faceBoxes, onFaceBoxes }: Props) {
  const closeRef = useRef<HTMLButtonElement>(null);
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
  // On a phone the panel is a sheet: drag its header down to put it away.
  const swipe = useVerticalSwipe({ onDown: onClose });
  // Focus lands on the panel when it opens (the lightbox hands it back to the Info button on close).
  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  return (
    <aside className="details-panel" id={panelId} aria-label="Photo details">
      <div className="details-head" {...swipe}>
        <span className="details-grip" aria-hidden="true" />
        <h2>Info</h2>
        <button type="button" className="btn btn-glass details-close" ref={closeRef} onClick={onClose}>
          Close info
        </button>
      </div>
      <div className="details-body">
        {state.status === "loading" && (
          <p className="muted" role="status">
            Loading details…
          </p>
        )}
        {state.status === "error" && (
          <p role="alert">
            Could not load the details.{" "}
            <button type="button" className="btn btn-glass" onClick={onRetry}>
              Try again
            </button>
          </p>
        )}
        {state.status === "ready" && (
          <Body d={state.details} follow={follow} onEdit={onEdit} rating={rating} onRate={onRate} tags={tags} onTag={onTag} onUntag={onUntag} onEditPicture={onEditPicture} faces={faces} dismissed={dismissed} onBringBack={onBringBack} faceBoxes={faceBoxes} onFaceBoxes={onFaceBoxes} />
        )}
      </div>
    </aside>
  );
}

function EditButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" className="btn btn-glass details-edit" aria-label={label} onClick={onClick}>
      {label.startsWith("Add") || label.startsWith("Edit") ? label.split(" ")[0] : "Adjust"}
    </button>
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
}) {
  const when = whenText(d.date);
  const loc = d.location;
  const place = loc?.place ? placeLine(loc.place) : "";
  const resolution = resolutionText(d.file);
  const x = d.exposure;
  const pair = d.files.length > 1;
  const named = peopleNamed(faces);

  return (
    <dl className="details-list">
      <Row label={d.date.basis === "taken" ? "Date" : d.date.basis === "file" ? "File date" : "Added"}>
        <PanelLink to={dayHref(d.date.timeline_day)} follow={follow} className="details-link">
          {when.day}
        </PanelLink>
        <span className="details-sub">
          {when.time}
          {when.time && " · "}
          <span className={d.date.zone_known ? undefined : "details-unknown"}>{when.zone}</span>
        </span>
        {when.note && <span className="details-sub">{when.note}</span>}
        {d.edits?.date_edited && <span className="details-sub details-edited">Edited</span>}
        {onEdit && <EditButton label="Adjust date and time" onClick={() => onEdit("date")} />}
      </Row>

      {onRate && (
        <Row label="Rating">
          <RatingStars value={rating} onChange={onRate} label="Rating of this photo" />
        </Row>
      )}

      {onTag && (
        <Row label="Tags">
          {tags && tags.length > 0 && (
            <span className="details-tags">
              {tags.map((t) => (
                <span key={t.id} className={`tag-chip${t.imported ? " imported" : ""}`} title={t.imported ? "From the photo's own keywords" : undefined}>
                  <PanelLink to={tagSearchHref(t.name)} follow={follow} className="tag-chip-name">
                    {t.name}
                  </PanelLink>
                  {onUntag && (
                    <button type="button" className="tag-chip-x" aria-label={`Remove tag ${t.name}`} onClick={() => onUntag(t)}>
                      ×
                    </button>
                  )}
                </span>
              ))}
            </span>
          )}
          <button type="button" className="btn btn-glass details-edit" onClick={onTag}>
            Add tag
          </button>
        </Row>
      )}

      {(named.length > 0 || (onFaceBoxes && (faces?.length ?? 0) > 0) || (onBringBack && (dismissed ?? 0) > 0)) && (
        <Row label="People">
          {named.length > 0 && (
            <span className="details-tags">
              {named.map((p) => (
                <span key={p.id} className="tag-chip person-chip">
                  <PanelLink to={`/people/${p.id}`} follow={follow} className="tag-chip-name">
                    {p.name}
                  </PanelLink>
                </span>
              ))}
            </span>
          )}
          {onFaceBoxes && (faces?.length ?? 0) > 0 && (
            <label className="details-check">
              <input type="checkbox" checked={faceBoxes === true} onChange={(e) => onFaceBoxes(e.target.checked)} /> Show face boxes on the photo
            </label>
          )}
          {onBringBack && (dismissed ?? 0) > 0 && (
            <button type="button" className="btn btn-glass details-edit" onClick={onBringBack}>
              Bring back {dismissed === 1 ? "1 detection" : `${dismissed} detections`} marked “Not a face”
            </button>
          )}
        </Row>
      )}

      {onEditPicture && (
        <Row label="Picture">
          <span className="details-sub">{recipeText(fromServer(d.edits?.recipe), d.file.width, d.file.height)}</span>
          {d.edits?.recipe && <span className="details-sub details-edited">Edited</span>}
          <button type="button" className="btn btn-glass details-edit" aria-label="Rotate and crop the picture" onClick={onEditPicture}>
            Crop
          </button>
        </Row>
      )}

      {loc && (
        <Row label="Location">
          {loc.place && (
            <PanelLink to={searchHref({ place: loc.place.city })} follow={follow} className="details-link">
              {place}
            </PanelLink>
          )}
          <span className="details-sub">{coordinatesText(loc.latitude, loc.longitude)}</span>
          {loc.altitude && <span className="details-sub">Altitude {loc.altitude}</span>}
          {loc.direction && <span className="details-sub">Facing {loc.direction}</span>}
          {d.edits?.location_edited && <span className="details-sub details-edited">Edited</span>}
          {onEdit && <EditButton label="Edit location" onClick={() => onEdit("location")} />}
          <MiniMap lat={loc.latitude} lon={loc.longitude} follow={follow} />
        </Row>
      )}
      {!loc && onEdit && (
        <Row label="Location">
          <EditButton label="Add location" onClick={() => onEdit("location")} />
        </Row>
      )}
      {(d.caption || onEdit) && (
        <Row label="Caption">
          {d.caption && <span className="details-caption">{d.caption}</span>}
          {onEdit && <EditButton label={d.caption ? "Edit caption" : "Add a caption"} onClick={() => onEdit("caption")} />}
        </Row>
      )}

      {d.camera?.label && (
        <Row label="Camera">
          <PanelLink to={searchHref({ camera: d.camera.label })} follow={follow} className="details-link">
            {d.camera.label}
          </PanelLink>
        </Row>
      )}
      {d.camera?.lens && (
        <Row label="Lens">
          <PanelLink to={searchHref({ lens: d.camera.lens })} follow={follow} className="details-link">
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

      <Row label={pair ? "Files" : "File"}>
        {d.files.map((f) => (
          <span key={f.id} className="details-file">
            <span className="details-name">{f.name}</span>
            <span className="details-sub">
              {f.format}
              {f.raw ? " (RAW)" : ""} · {fileSize(f.size_bytes)}
            </span>
          </span>
        ))}
      </Row>
      {resolution && <Row label="Resolution">{resolution}</Row>}
      {d.file.color_space && <Row label="Colour space">{d.file.color_space}</Row>}
      {d.file.software && <Row label="Software">{d.file.software}</Row>}

      {d.albums.length > 0 && (
        <Row label="Albums">
          {d.albums.map((a) => (
            <PanelLink key={a.id} to={`/albums/${a.id}`} follow={follow} className="details-link">
              {a.title}
            </PanelLink>
          ))}
        </Row>
      )}
      {d.burst && <Row label="Burst">{burstText(d.burst)}</Row>}
    </dl>
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
    <PanelLink to={mapHref(lat, lon)} follow={follow} className="place-map-link" aria-label="Open the map at this photo">
      <Suspense fallback={<div className="place-map" />}>
        {tiles ? <PlaceMap lat={lat} lon={lon} basemap={tiles} dark={dark} onUnavailable={() => setFailed(true)} /> : <div className="place-map" />}
      </Suspense>
    </PanelLink>
  );
}
