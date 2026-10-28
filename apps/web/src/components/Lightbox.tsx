import { useNavigate } from "react-router-dom";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type MouseEvent } from "react";
import { flushSync } from "react-dom";
import { animate, motion, useMotionValue, useReducedMotion } from "motion/react";
import {
  Archive,
  ArchiveRestore,
  ChevronLeft,
  ChevronRight,
  Crop,
  Download,
  Ellipsis,
  Eye,
  EyeOff,
  FolderPlus,
  Heart,
  Images,
  Info,
  Play,
  ScanText,
  Share,
  Trash2,
} from "lucide-react";
import { api, downloadHref, editedHref, type AssetDetail, type AssetListItem, type DismissedFace, type PhotoDetails, type PhotoFace, type PhotoPet, type PhotoTag, type PhotoText } from "../api";
import { usePeopleEnabled } from "../usePeople";
import { usePetsEnabled } from "../usePets";
import { removedFromPetMessage } from "../petsText";
import { NamePetSheet, petNamedToast } from "./NamePetSheet";
import { AssignFaceDialog } from "./FaceChips";
import { FaceRegions, namedFace } from "./FaceRegions";
import { useToast } from "./Toast";
import { NOT_A_FACE_MESSAGE } from "../peopleText";
import { useInfoHistory } from "../useInfoPanel";
import { isFlick, rubber, useViewerDrag } from "../useViewerGestures";
import { useIdleChrome } from "../useIdleChrome";
import { useImageRect, useTextPanel } from "../useTextPanel";
import { TextBoxes, TextPanel } from "./TextPanel";
import { DetailsPanel, type DetailsState } from "./DetailsPanel";
import { EditDialog, type EditKind } from "./EditDialog";
import { placeLine } from "../map/geo";
import { fileSize, pairLabel, primaryFormat } from "../pairText";
import { placeholderUrl } from "../thumbhash";
import { RatingStars } from "./Stars";
import { TagDialog } from "./TagDialog";
import { ratingKey } from "../ratingText";
import { fromServer, toRequest, type Recipe } from "../recipeText";
import { RotateCrop } from "./RotateCrop";
import { fitStyle } from "../photoFit";
import { usePhotoZoom } from "../usePhotoZoom";
import { sharperUrl } from "../zoomMath";
import { ZoomControls } from "./ZoomControls";
import { itemsSource } from "../slideshow/sources";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { StackBar, type StackView } from "./StackBar";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { Menu, MenuItem, MenuLabel, MenuSeparator } from "../ui/Menu";
import { tileRect } from "../ui/sharedTile";
import { useDesktop } from "../ui/useMedia";
import { CLOSE_MS, EASE, LIFT_REST, OPEN_MS, liftOnto, onScreen } from "../viewerMotion";
import "../viewer.css";

/** The keys that zoom the photo: + and = in, - and _ out, z between fit and zoomed. */
const ZOOM_KEYS = ["+", "=", "-", "_", "z"];

/** How often the viewer asks whether the worker has finished a picture or an edited download, and how many times. */
const POLL_MS = 1500;
const POLL_TRIES = 40;

/** The gap between photos in the paging strip (px). */
const GAP = 24;
/** A drag this far (px) closes or opens the info; a quick flick needs less (see `isFlick`). */
const SWIPE_PX = 96;
const OPEN_INFO_PX = 64;

const SPRING = { type: "spring", stiffness: 420, damping: 38, mass: 0.9 } as const;

interface Props {
  items: AssetListItem[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  loadDetail: (id: string) => Promise<AssetDetail>;
  /** Read the photo's details (the Info panel: date, place, camera, exposure, files). The Info button shows when given. */
  loadDetails?: (id: string) => Promise<PhotoDetails>;
  /** Read the text found in the photo (the Text panel: copy it, QR codes). Defaults to the server's; the Text item shows when `loadDetails` is given. */
  loadText?: (id: string) => Promise<PhotoText>;
  /** Move the current photo to the trash. The parent removes it from `items`, which shows the next photo. */
  onTrash?: (item: AssetListItem) => void;
  /** Open the "add to album" dialog for the current photo. */
  onAddToAlbum?: (item: AssetListItem) => void;
  /** Open the "share link" dialog for the current photo. */
  onShare?: (item: AssetListItem) => void;
  /** Separate the current photo from its RAW (offered in the pair info when the photo has one). */
  onUnpair?: (item: AssetListItem) => void;
  /** Show photos like the current one ("Similar photos"). */
  onSimilar?: (item: AssetListItem) => void;
  /** Favorite or unfavorite the current photo (the parent updates `item.favorite`). Shows the heart when given. */
  onFavorite?: (item: AssetListItem, favorite: boolean) => void;
  /** Rate the current photo 0 to 5 stars (keys 0 to 5, the stars under the photo and in the info panel). Shown when given. */
  onRate?: (item: AssetListItem, rating: number) => void;
  /** Archive the current photo, or restore it when it is archived (the menu says which). Shown when given. */
  onArchive?: (item: AssetListItem) => void;
  /** Hide the current photo, or unhide it when it is hidden (unhiding needs the unlocked Hidden view). Shown when given. */
  onHide?: (item: AssetListItem) => void;
  /** Position of `items[0]` in the whole list and the size of the whole list, when `items` is only the part that is
   * loaded around the current photo (the counter and the end stops use them). */
  offset?: number;
  total?: number;
  /** The photo's date, location or caption was edited (the page refreshes what depends on them: the timeline order...). */
  onEdited?: (item: AssetListItem) => void;
  /** Bump after the photo's albums changed so its details (the "In albums" line) are fetched again. */
  refreshKey?: number;
  /** How often to ask whether the worker has finished rendering an edited picture or an edited download (tests shorten it). */
  pollMs?: number;
  /** The photo on show is part of a stack: the strip under it, `[` and `]` to step through the stack (the arrows still move through `items`). */
  stack?: StackView;
}

interface PhotoSize {
  w: number;
  h: number;
  preview: boolean;
}

/** The photo viewer: an immersive dark layer over the page with bars that fade away, paging and closing by dragging, and zoom. */
export function Lightbox({
  items,
  index,
  onIndexChange,
  onClose,
  loadDetail,
  loadDetails,
  loadText = api.getText,
  onEdited,
  onTrash,
  onAddToAlbum,
  onShare,
  onUnpair,
  onSimilar,
  onFavorite,
  onRate,
  onArchive,
  onHide,
  offset = 0,
  total = items.length,
  refreshKey = 0,
  pollMs = POLL_MS,
  stack,
}: Props) {
  const item = items[index];
  const desktop = useDesktop();
  const reduce = useReducedMotion();
  const [details, setDetails] = useState<Record<string, AssetDetail>>({});
  const [failed, setFailed] = useState<Record<string, true>>({});
  const requested = useRef(new Set<string>());
  const dialogRef = useRef<HTMLDivElement>(null);
  // Which photos' pictures have loaded, and how big they are (the CSS fits each photo to the area from its shape).
  const [loaded, setLoaded] = useState<Record<string, true>>({});
  const [sizes, setSizes] = useState<Record<string, PhotoSize>>({});

  // The Info panel: swipe up on the photo (phone), the Info button or `i`. It stays open while paging through photos.
  const info = useInfoHistory();
  const [infoById, setInfoById] = useState<Record<string, PhotoDetails>>({});
  const [infoFailed, setInfoFailed] = useState<Record<string, true>>({});
  const infoRequested = useRef(new Set<string>());
  const infoButton = useRef<HTMLButtonElement>(null);
  const currentId = items[index]?.id;
  const [infoRetry, setInfoRetry] = useState(0);
  useEffect(() => {
    if (!info.open || !loadDetails || !currentId || infoRequested.current.has(currentId)) return;
    infoRequested.current.add(currentId);
    const id = currentId;
    loadDetails(id)
      .then((d) => setInfoById((m) => ({ ...m, [id]: d })))
      .catch(() => {
        infoRequested.current.delete(id);
        setInfoFailed((m) => ({ ...m, [id]: true }));
      });
  }, [info.open, loadDetails, currentId, infoRetry]);
  // The faces in the photo (when People is on): regions over the picture (a named one opens the person, any other asks "Who is this?"),
  // and the named people in the Info panel. The owner's own session only: nothing here is loaded without People, or in a share.
  const peopleOn = usePeopleEnabled() === true;
  // (The router is only needed once People is on, and is reached through a child: the viewer is also drawn without one.)
  const navigateRef = useRef<((to: string) => void) | null>(null);
  const { toast, show: showToast } = useToast();
  const [facesById, setFacesById] = useState<Record<string, PhotoFace[]>>({});
  const [dismissedById, setDismissedById] = useState<Record<string, DismissedFace[]>>({});
  const [namingFace, setNamingFace] = useState<PhotoFace | null>(null);
  const [facesVersion, setFacesVersion] = useState(0);
  const [faceBoxes, setFaceBoxes] = useState(false);
  useEffect(() => {
    if (!peopleOn || !loadDetails || !currentId) return;
    let live = true;
    const id = currentId;
    api.photoFaces(id).then(
      (r) => {
        if (!live) return;
        setFacesById((m) => ({ ...m, [id]: r.items }));
        setDismissedById((m) => ({ ...m, [id]: r.dismissed ?? [] }));
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [peopleOn, loadDetails, currentId, facesVersion]);
  const faceHere = currentId ? facesById[currentId] : undefined;
  // The animals in the photo (when Pets is on): the Info panel lists them with the named pet each is in, to name or take out.
  const petsOn = usePetsEnabled() === true;
  const [animalsById, setAnimalsById] = useState<Record<string, PhotoPet[]>>({});
  const [animalsVersion, setAnimalsVersion] = useState(0);
  const [namingAnimal, setNamingAnimal] = useState<PhotoPet | null>(null);
  useEffect(() => {
    if (!petsOn || !loadDetails || !currentId) return;
    let live = true;
    const id = currentId;
    api.assetPets(id).then(
      (r) => live && setAnimalsById((m) => ({ ...m, [id]: r.items })),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [petsOn, loadDetails, currentId, animalsVersion]);
  const notThisPet = async (animal: PhotoPet) => {
    if (!animal.pet) return;
    const { id, name } = animal.pet;
    try {
      await api.removeFromPet(id, { detection_ids: [animal.id] });
      showToast({ message: removedFromPetMessage(1, name) });
    } catch (e) {
      showToast({ tone: "error", message: `Couldn’t take it out. ${e instanceof Error ? e.message : ""}`.trim() });
    }
    setAnimalsVersion((v) => v + 1);
  };
  const animalIs = async (animal: PhotoPet, kind: "dog" | "cat") => {
    try {
      await api.setPetDetectionKind(animal.id, kind);
      showToast({ message: animal.pet && animal.kind !== kind ? `Marked as a ${kind}, and taken out of ${animal.pet.name}` : `Marked as a ${kind}` });
    } catch (e) {
      showToast({ tone: "error", message: `Couldn’t save that. ${e instanceof Error ? e.message : ""}`.trim() });
    }
    setAnimalsVersion((v) => v + 1);
  };
  const openFace = (face: PhotoFace) => {
    if (namedFace(face)) void info.leave().then(() => {
      onClose();
      navigateRef.current?.(`/people/${face.person!.id}`);
    });
    else setNamingFace(face);
  };
  const undoNotAFace = async (face: string, was: string | null) => {
    try {
      await api.unmarkNotAFace(face);
      if (was) await api.assignFace(face, { person_id: was });
    } catch {
      showToast({ tone: "error", message: "Couldn’t undo that." });
    }
    setFacesVersion((v) => v + 1);
  };
  const markedNotAFace = (face: PhotoFace, was: string | null) => {
    setNamingFace(null);
    setFacesVersion((v) => v + 1);
    showToast({ message: NOT_A_FACE_MESSAGE, action: { label: "Undo", onClick: () => void undoNotAFace(face.id, was) } });
  };
  const notAFace = async (face: PhotoFace) => {
    try {
      markedNotAFace(face, (await api.markNotAFace(face.id)).person_id);
    } catch (e) {
      showToast({ tone: "error", message: `Couldn’t save that. ${e instanceof Error ? e.message : ""}`.trim() });
    }
  };
  const bringBack = async (faces: DismissedFace[]) => {
    try {
      await Promise.all(faces.map((f) => api.unmarkNotAFace(f.id)));
    } catch {
      showToast({ tone: "error", message: "Couldn’t bring them back." });
    }
    setFacesVersion((v) => v + 1);
  };
  const closeInfo = info.hide;
  // The Text panel: the Text item of the menu or `t`. It takes the Info panel's place (one panel at a time) and, like it, stays open while paging.
  const text = useTextPanel(loadDetails ? loadText : undefined, currentId);
  const { close: closeText } = text;
  useEffect(() => {
    if (info.open) closeText();
  }, [info.open, closeText]);
  const toggleText = () => {
    if (!text.open) closeInfo();
    text.toggle();
  };
  const stageRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const photoRef = useRef<HTMLDivElement>(null);
  // Zoom: the preview's size, and the sharper picture (the original file of an unedited JPEG, PNG or WebP) to swap in once the
  // zoom goes past what the preview shows. Edited photos, HEIC and RAW have none: their preview is simply scaled.
  const levelRef = useRef<HTMLSpanElement>(null);
  const announceRef = useRef<HTMLSpanElement>(null);
  const here0 = items[index];
  const detail0 = currentId ? details[currentId] : undefined;
  const baseSize = currentId ? sizes[currentId] : undefined;
  // (Only the preview counts: the thumbnail shown while it loads says nothing about what the preview can resolve.)
  const previewLong = baseSize?.preview ? Math.max(baseSize.w, baseSize.h) : 0;
  const sharpUrl =
    here0 && detail0?.preview_url
      ? sharperUrl({
          contentType: here0.content_type,
          edited: here0.edited === true || !!detail0.recipe,
          rendering: detail0.rendering === true,
          originalUrl: detail0.original_url,
          width: detail0.file_width ?? here0.width,
          height: detail0.file_height ?? here0.height,
          previewLong,
        })
      : null;
  const zoom = usePhotoZoom({
    stage: stageRef,
    layer: photoRef,
    photo: imgRef,
    level: levelRef,
    announce: announceRef,
    resetKey: currentId,
    active: !!here0,
    previewLong,
    naturalLong: sharpUrl ? Math.max(detail0?.file_width ?? here0?.width ?? 0, detail0?.file_height ?? here0?.height ?? 0) : previewLong,
    // A tap on a face on the photo (heard here so that gestures never open it): the face the region stands for.
    onTap: (el) => {
      const face = faceHere?.find((f) => f.id === el.dataset.faceId);
      if (face) openFace(face);
    },
  });
  const [sharpReady, setSharpReady] = useState<string | null>(null);
  const wantSharp = zoom.wantSharp;
  useEffect(() => {
    if (!wantSharp || !sharpUrl) return;
    let live = true;
    const probe = new Image();
    probe.src = sharpUrl;
    // Swap once the sharper picture is decoded, so the photo never blanks; if it cannot be loaded the preview stays.
    probe.decode().then(
      () => live && setSharpReady(sharpUrl),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [wantSharp, sharpUrl]);
  const showingSharp = wantSharp && !!sharpUrl && sharpReady === sharpUrl;
  const imageRect = useImageRect(stageRef, imgRef, [currentId, currentId ? loaded[currentId] : false, text.open, text.boxes, info.open]);
  // The editor (date, location, caption) of the open photo, from the panel.
  const [editing, setEditing] = useState<EditKind | null>(null);
  const [editVersion, setEditVersion] = useState(0);
  // Adding a tag to the open photo (the dialog), and a failed removal.
  const [tagging, setTagging] = useState(false);
  const [tagError, setTagError] = useState<string | null>(null);
  // The rotate/crop editor of the open photo (the Edit button, or `e`, `r`, `c`), and the photo the worker is rendering after a save.
  const [picture, setPicture] = useState<{ cropping: boolean; turned: boolean } | null>(null);
  const [waitingFor, setWaitingFor] = useState<string | null>(null);
  // "Download edited": the photo being prepared, and why it failed.
  const [preparing, setPreparing] = useState<string | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // The bars: shown on any interaction, gone after a quiet spell, back on a tap or a mouse move.
  const chrome = useIdleChrome(dialogRef, info.open || text.open || picture !== null || editing !== null || tagging);
  const { show: showChrome } = chrome;

  // ---- Open, close and drag-to-close: the photo box is lifted (moved, scaled, clipped) onto the grid's tile and back. ----
  const bg = useMotionValue(0);
  const fade = useMotionValue(0);
  const chromeFade = useMotionValue(0);
  const lx = useMotionValue(0);
  const ly = useMotionValue(0);
  const ls = useMotionValue(1);
  const clip = useMotionValue(LIFT_REST.clip);
  const trackX = useMotionValue(0);
  const closing = useRef(false);

  const ratioOf = (it: AssetListItem | undefined) => (it?.width && it?.height ? it.width / it.height : 3 / 2);
  const tileLift = useCallback((id: string, ratio: number) => {
    const box = boxRef.current?.getBoundingClientRect();
    const r = tileRect(id);
    return box && r && onScreen(r) ? liftOnto(r, box, ratio) : null;
  }, []);
  const apply = useCallback(
    (to: { x: number; y: number; scale: number; clip: string }, ms: number, bgTo: number, fadeTo = 1) => {
      const o = { duration: ms, ease: EASE };
      return Promise.all([animate(lx, to.x, o), animate(ly, to.y, o), animate(ls, to.scale, o), animate(clip, to.clip, o), animate(bg, bgTo, o), animate(fade, fadeTo, o)]);
    },
    [lx, ly, ls, clip, bg, fade],
  );

  useLayoutEffect(() => {
    const first = items[index];
    const lift = first && !reduce ? tileLift(first.id, ratioOf(first)) : null;
    const o = { duration: reduce ? 0.12 : OPEN_MS, ease: EASE };
    if (lift) {
      lx.set(lift.x);
      ly.set(lift.y);
      ls.set(lift.scale);
      clip.set(lift.clip);
      fade.set(1);
      void apply(LIFT_REST, OPEN_MS, 1);
      void animate(chromeFade, 1, { ...o, delay: 0.18 });
    } else {
      // No tile to grow from (or less motion wanted): a fade.
      void animate(bg, 1, o);
      void animate(fade, 1, o);
      void animate(chromeFade, 1, o);
    }
    // Once, when the viewer opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Close: take the info's history entry off, then shrink the photo into its tile (or fade) and tell the parent. */
  const close = useCallback(async () => {
    if (closing.current) return;
    closing.current = true;
    await info.leave();
    const cur = items[index];
    const lift = cur && !reduce && !zoom.zoomed && Math.abs(trackX.get()) < 1 ? tileLift(cur.id, ratioOf(cur)) : null;
    void animate(chromeFade, 0, { duration: 0.12 });
    if (lift) await apply(lift, CLOSE_MS, 0);
    else {
      const o = { duration: reduce ? 0.1 : 0.18, ease: EASE };
      // Dragged down and let go: carry on in the direction of the drag.
      const away = !reduce && ly.get() > 40 ? animate(ly, window.innerHeight * 0.6, o) : null;
      await Promise.all([animate(bg, 0, o), animate(fade, 0, o), away]);
    }
    onClose();
    // (`index` and `items` are read when called: the latest render's close is the one wired up.)
  }, [info, items, index, reduce, zoom.zoomed, trackX, tileLift, apply, chromeFade, bg, fade, ly, onClose]);

  const springBack = useCallback(() => {
    const o = reduce ? { duration: 0 } : SPRING;
    void animate(lx, 0, o);
    void animate(ly, 0, o);
    void animate(ls, 1, o);
    void animate(bg, 1, o);
    void animate(chromeFade, 1, { duration: 0.15 });
  }, [reduce, lx, ly, ls, bg, chromeFade]);

  // ---- Paging: the neighbours sit either side of the photo, the whole strip follows the finger. ----
  const go = useCallback(
    (delta: number) => {
      const next = index + delta;
      if (next >= 0 && next < items.length) onIndexChange(next);
    },
    [index, items.length, onIndexChange],
  );
  const hasPrev = index > 0;
  const hasNext = index < items.length - 1;

  const drag = useViewerDrag({
    enabled: !!here0 && !zoom.zoomed && picture === null && editing === null && !tagging,
    onMove: ({ axis, dx, dy }) => {
      if (closing.current) return;
      showChrome();
      if (axis === "h") {
        trackX.set((dx > 0 && !hasPrev) || (dx < 0 && !hasNext) ? rubber(dx) : dx);
      } else if (dy > 0) {
        // Down: the photo follows, shrinks, and the backdrop and bars fade with it.
        const h = window.innerHeight || 800;
        ly.set(dy);
        lx.set(dx * 0.6);
        ls.set(1 - Math.min(dy / (h * 1.5), 0.35));
        bg.set(1 - Math.min(dy / (h * 0.45), 1));
        chromeFade.set(1 - Math.min(dy / 120, 1));
      } else {
        ly.set(dy * 0.25);
      }
    },
    onEnd: ({ axis, dx, dy, ms, cancelled }) => {
      if (closing.current) return;
      if (axis === "h") {
        const w = stageRef.current?.clientWidth || window.innerWidth || 1;
        const dir = dx < 0 ? 1 : -1;
        const possible = dir === 1 ? hasNext : hasPrev;
        const far = Math.abs(dx) > w * 0.25 || isFlick(dx, ms, 40);
        if (!cancelled && possible && far) {
          const target = -dir * (w + GAP);
          const done = () => {
            flushSync(() => onIndexChange(index + dir));
            trackX.set(0);
          };
          if (reduce) done();
          else void animate(trackX, target, SPRING).then(done);
        } else void animate(trackX, 0, reduce ? { duration: 0 } : SPRING);
      } else if (!cancelled && dy > 0 && (dy > SWIPE_PX || isFlick(dy, ms, 40))) {
        void close();
      } else if (!cancelled && dy < 0 && loadDetails && !info.open && (dy <= -OPEN_INFO_PX || isFlick(dy, ms))) {
        springBack();
        info.show();
      } else springBack();
    },
  });
  // The new photo takes the old one's place before anything is painted.
  useLayoutEffect(() => {
    trackX.set(0);
  }, [index, trackX]);

  // Closing the panel gives focus back to the button that opens it.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (wasOpen.current && !info.open) infoButton.current?.focus();
    wasOpen.current = info.open;
  }, [info.open]);

  // Fetch fresh presigned URLs for the current photo and its neighbours so ←/→ feels instant.
  useEffect(() => {
    for (const i of [index, index + 1, index - 1]) {
      const id = items[i]?.id;
      if (!id || requested.current.has(id)) continue;
      requested.current.add(id);
      loadDetail(id)
        .then((d) => setDetails((m) => ({ ...m, [id]: d })))
        .catch(() => {
          requested.current.delete(id);
          setFailed((m) => ({ ...m, [id]: true }));
        });
    }
  }, [index, items, loadDetail]);

  // Albums changed: re-read the current photo's details (fresh URLs and album list), keeping the old ones meanwhile.
  const refreshTotal = refreshKey + editVersion;
  const lastRefresh = useRef(refreshTotal);
  useEffect(() => {
    if (lastRefresh.current === refreshTotal) return;
    lastRefresh.current = refreshTotal;
    const id = items[index]?.id;
    if (!id) return;
    loadDetail(id)
      .then((d) => setDetails((m) => ({ ...m, [id]: d })))
      .catch(() => undefined);
    // (The panel's album list is read again too.)
    if (loadDetails && infoRequested.current.has(id)) {
      loadDetails(id)
        .then((d) => setInfoById((m) => ({ ...m, [id]: d })))
        .catch(() => undefined);
    }
  }, [refreshTotal, items, index, loadDetail, loadDetails]);

  // After a save the worker renders the picture again: ask until it has, then show it (and let the page refresh its tile).
  useEffect(() => {
    if (!waitingFor) return;
    let tries = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const id = waitingFor;
    const tick = () => {
      tries += 1;
      loadDetail(id)
        .then((d) => {
          setDetails((m) => ({ ...m, [id]: d }));
          if (d.rendering && tries < POLL_TRIES) timer = setTimeout(tick, pollMs);
          else {
            setWaitingFor(null);
            const cur = items.find((i) => i.id === id);
            if (cur) onEdited?.(cur);
          }
        })
        .catch(() => {
          if (tries < POLL_TRIES) timer = setTimeout(tick, pollMs);
          else setWaitingFor(null);
        });
    };
    tick();
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- one polling run per save
  }, [waitingFor]);

  const slideshow = useSlideshow();
  const playFromHere = useCallback(() => {
    const cur = items[index];
    if (slideshow && cur) slideshow.start(itemsSource(items, "Photos", cur.id));
  }, [slideshow, items, index]);

  // Rotate and crop: the editor works on the whole upright picture, which is the preview unless the photo is already cropped.
  const here = items[index];
  const hereDetail = here ? details[here.id] : undefined;
  const editBase = hereDetail ? (hereDetail.recipe ? (hereDetail.edit_base_url ?? null) : hereDetail.preview_url) : null;
  const canEditPicture = !!loadDetails && !!editBase && hereDetail?.rendering !== true;
  const pictureOpen = picture !== null;

  // Keyboard: ←/→ navigate, Esc closes, Tab stays inside the dialog.
  useEffect(() => {
    /** The key is the viewer's: not typed into a field, and not meant for a dialog or menu opened over it (add to album, share, info sheet). */
    const owns = (e: KeyboardEvent, inPanels = true) => {
      const t = e.target as HTMLElement | null;
      if (t?.closest("input, textarea, select")) return false;
      // The viewer's own panels (Info, Text; a sheet on a phone) are part of it: its keys work with the keyboard in them.
      if (inPanels && t?.closest("#details-panel, #text-panel")) return true;
      const over = t?.closest('[role="dialog"], dialog, [role="menu"]') ?? null;
      return over === null || over === dialogRef.current;
    };
    function onKey(e: KeyboardEvent) {
      // Typing in a dialog opened over the lightbox (add to album) is not a lightbox shortcut.
      if ((e.target as HTMLElement | null)?.closest("input, textarea, select")) return;
      // The rotate/crop editor owns the keyboard while it is open.
      if (pictureOpen) return;
      // Any key brings the bars back.
      showChrome();
      const bare = !e.ctrlKey && !e.metaKey && !e.altKey;
      if (loadDetails && ["e", "r", "c"].includes(e.key) && bare && !e.repeat && owns(e)) {
        const cur = items[index];
        if (!cur || !canEditPicture) return;
        e.preventDefault();
        setPicture({ cropping: e.key === "c", turned: e.key === "r" });
        return;
      }
      // Zoom: + and = in, - out, z between fit and zoomed. (0 to 5 are the rating, and Ctrl with +/- is the browser's page zoom.)
      if (ZOOM_KEYS.includes(e.key) && bare && owns(e)) {
        e.preventDefault();
        if (e.key === "+" || e.key === "=") zoom.zoomIn();
        else if (e.key === "-" || e.key === "_") zoom.zoomOut();
        else if (!e.repeat) zoom.toggle();
        return;
      }
      if (e.key === "Escape") {
        // A menu or sheet handles its own Escape (and gives focus back); the panel closes first, the next Escape closes the viewer.
        const t = e.target as HTMLElement | null;
        if (t?.closest('[role="menu"], dialog')) return;
        const over = t?.closest('[role="dialog"]') ?? null;
        if (over !== null && over !== dialogRef.current) return;
        // Another sheet is open and focus is on the page behind it (the field it was on went away): its Escape is not the viewer's.
        if (!info.open && !text.open && document.querySelector("dialog[open]")) return;
        e.preventDefault();
        if (info.open) closeInfo();
        else if (text.open) closeText();
        else void close();
      } else if (e.key === "i" && loadDetails && bare && !e.repeat && owns(e)) {
        e.preventDefault();
        info.toggle();
      } else if (e.key === "s" && slideshow && bare && !e.repeat && owns(e)) {
        e.preventDefault();
        playFromHere();
      } else if (e.key === "t" && text.enabled && bare && !e.repeat && owns(e)) {
        e.preventDefault();
        if (!text.open) closeInfo();
        text.toggle();
      } else if (onRate && ratingKey(e) !== null && !e.repeat && owns(e)) {
        const cur = items[index];
        if (!cur) return;
        e.preventDefault();
        onRate(cur, ratingKey(e)!);
      } else if (stack && (e.key === "[" || e.key === "]") && bare && owns(e)) {
        e.preventDefault();
        stack.onStep(e.key === "[" ? -1 : 1);
      } else if (e.key === "ArrowLeft" && owns(e)) {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight" && owns(e)) {
        e.preventDefault();
        go(1);
      } else if ((e.key === "Delete" || e.key === "Backspace") && onTrash && !e.repeat && owns(e)) {
        const cur = items[index];
        if (!cur) return;
        e.preventDefault();
        onTrash(cur);
      } else if (e.key === "Tab" && dialogRef.current && owns(e, false)) {
        const f = dialogRef.current.querySelectorAll<HTMLElement>("button:not(:disabled), a[href]");
        const first = f[0];
        const last = f[f.length - 1];
        if (!first || !last) return;
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [go, close, onTrash, onRate, items, index, info, closeInfo, loadDetails, text, closeText, pictureOpen, canEditPicture, zoom, slideshow, playFromHere, stack, showChrome]);

  // Focus management: move focus in, give it back to the tile on close; lock page scroll.
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    dialogRef.current?.querySelector<HTMLElement>("button")?.focus();
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = overflow;
      prev?.focus?.();
    };
  }, []);

  // A tap on the photo (a touch, not a face or a double tap) shows or hides the bars.
  const lastPointer = useRef("mouse");
  const tapTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(tapTimer.current), []);
  const onStageClick = (e: MouseEvent) => {
    if (lastPointer.current === "mouse" || (e.target as HTMLElement).closest("[data-tap], button, a")) return;
    // A second tap soon after is the double tap that zooms: the bars stay as they are.
    if (tapTimer.current) {
      clearTimeout(tapTimer.current);
      tapTimer.current = undefined;
      return;
    }
    tapTimer.current = setTimeout(() => {
      tapTimer.current = undefined;
      chrome.toggle();
    }, 280);
  };

  if (!item) return null;
  const detail = details[item.id];
  const date = new Date(item.sort_time);
  const validDate = !Number.isNaN(date.getTime());
  const paired = detail?.paired ?? item.paired ?? [];
  const favorite = item.favorite === true;
  // The files that can be saved: the photo itself and, for a RAW + JPEG pair, its RAW.
  const files = [
    { key: item.id, href: downloadHref(item.id), label: paired.length > 0 ? primaryFormat(item) : "", size: item.size_bytes },
    ...paired.map((p) => ({ key: p.id, href: downloadHref(item.id, p.id), label: "RAW", size: p.size_bytes })),
  ];
  const position = offset + index;
  const rendering = detail?.rendering === true;
  const edited = item.edited === true;
  const pixelSize: [number, number] | null =
    detail?.file_width && detail.file_height
      ? [detail.file_width, detail.file_height]
      : !edited && item.width && item.height
        ? [item.width, item.height]
        : null;
  async function savePicture(recipe: Recipe) {
    const res = await api.editAssets([item!.id], { recipe: toRequest(recipe) });
    if (res.failed.length > 0 || res.missing > 0) throw new Error("Could not save the changes. Try again.");
    finishPicture();
  }
  async function revertPicture() {
    const res = await api.editAssets([item!.id], { recipe: { revert: true } });
    if (res.failed.length > 0 || res.missing > 0) throw new Error("Could not go back to the original. Try again.");
    finishPicture();
  }
  function finishPicture() {
    setPicture(null);
    setEditVersion((v) => v + 1);
    setWaitingFor(item!.id);
    onEdited?.(item!);
  }
  async function downloadEdited() {
    const id = item!.id;
    setPreparing(id);
    setDownloadError(null);
    try {
      for (let i = 0; i < POLL_TRIES && alive.current; i++) {
        if ((await api.editedStatus(id)) === "ready") {
          window.location.assign(editedHref(id));
          break;
        }
        await new Promise((r) => setTimeout(r, pollMs));
      }
    } catch (e) {
      if (alive.current) setDownloadError(e instanceof Error ? e.message : "Could not prepare the edited photo.");
    } finally {
      if (alive.current) setPreparing(null);
    }
  }
  async function untag(t: PhotoTag) {
    setTagError(null);
    try {
      await api.applyTags([item!.id], { remove: [t.name] });
      setEditVersion((v) => v + 1);
    } catch (e) {
      setTagError(e instanceof Error ? e.message : "Could not remove the tag.");
    }
  }
  const retryInfo = () => {
    setInfoFailed((m) => Object.fromEntries(Object.entries(m).filter(([k]) => k !== item.id)));
    setInfoRetry((n) => n + 1);
  };
  const infoState: DetailsState = infoById[item.id]
    ? { status: "ready", details: infoById[item.id]! }
    : infoFailed[item.id]
      ? { status: "error" }
      : { status: "loading" };

  const src = detail?.preview_url ?? null;
  const fallback = detail?.thumb_url ?? item.thumb_url;
  const bgUrl = placeholderUrl(item.thumbhash);
  const showPending = detail && !src;

  // One photo of the strip: the current one carries the zoom layer, its outlines and the sharper picture; the neighbours are previews.
  const slot = (offsetBy: -1 | 0 | 1) => {
    const it = items[index + offsetBy];
    if (!it) return null;
    const d = details[it.id];
    const cur = offsetBy === 0;
    const s = d?.preview_url ?? null;
    const fb = d?.thumb_url ?? it.thumb_url;
    const url = (cur && showingSharp ? sharpUrl : null) ?? s ?? fb;
    const fit = sizes[it.id];
    return (
      <div
        key={it.id}
        className="viewer-slot"
        style={{ transform: offsetBy === 0 ? undefined : `translateX(calc(${offsetBy * 100}% + ${offsetBy * GAP}px))` }}
        aria-hidden={cur ? undefined : true}
      >
        <div className={cn("viewer-photo", cur && "current")} ref={cur ? photoRef : undefined}>
          {url && (
            <img
              ref={cur ? imgRef : undefined}
              src={url}
              alt={cur ? it.original_name : ""}
              className={`${s && loaded[it.id] ? "loaded" : s ? "loading" : "loaded"}${fit ? " sized" : ""}`}
              style={fitStyle(fit)}
              draggable={false}
              onLoad={(e) => {
                const img = e.currentTarget;
                setLoaded((m) => (m[it.id] ? m : { ...m, [it.id]: true }));
                // The sharper picture is drawn in the box the preview made: its own size must not change the layout.
                if (!(cur && showingSharp)) setSizes((m) => ({ ...m, [it.id]: { w: img.naturalWidth, h: img.naturalHeight, preview: !!s } }));
              }}
            />
          )}
          {cur && text.open && text.boxes && text.current && imageRect && <TextBoxes text={text.current} style={imageRect} />}
          {cur && peopleOn && loaded[it.id] && faceHere && faceHere.length > 0 && imageRect && (
            <FaceRegions faces={faceHere} rect={imageRect} showAll={faceBoxes} onOpen={openFace} onNotAFace={(f) => void notAFace(f)} />
          )}
        </div>
      </div>
    );
  };

  const shownBars = chrome.visible;
  // The bars let touches through to the photo except where a control is: only the controls take pointer events, and not while hidden.
  const barClass = cn("pointer-events-none transition-opacity duration-200", shownBars ? "opacity-100" : "opacity-0");
  const auto = shownBars ? "pointer-events-auto" : "";
  const placeText = detail?.place ? placeLine(detail.place) : "";
  const timeText = validDate ? date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) : "";

  // The icon buttons: favorite, info, share, rotate and crop, trash. On a phone they are the bottom bar; on a desktop the top right.
  const actions = (
    <>
      {onFavorite && (
        <Button
          variant="overlay"
          size="icon"
          aria-pressed={favorite}
          aria-label={favorite ? "Remove from favorites" : "Add to favorites"}
          onClick={() => onFavorite(item, !favorite)}
        >
          <Heart aria-hidden="true" className={favorite ? "fill-current" : undefined} />
        </Button>
      )}
      {loadDetails && (
        <Button
          variant="overlay"
          size="icon"
          ref={infoButton}
          className={info.open ? "bg-white/30" : undefined}
          aria-label="Info"
          aria-expanded={info.open}
          aria-controls="details-panel"
          aria-keyshortcuts="i"
          onClick={info.toggle}
        >
          <Info aria-hidden="true" />
        </Button>
      )}
      {onShare && (
        <Button variant="overlay" size="icon" aria-label="Share" onClick={() => onShare(item)}>
          <Share aria-hidden="true" />
        </Button>
      )}
      {loadDetails && (
        <Button
          variant="overlay"
          size="icon"
          aria-label="Rotate and crop"
          aria-keyshortcuts="e"
          disabled={!canEditPicture}
          title={rendering ? "The picture is being updated" : "Rotate and crop"}
          onClick={() => setPicture({ cropping: false, turned: false })}
        >
          <Crop aria-hidden="true" />
        </Button>
      )}
      {onTrash && (
        <Button variant="overlay" size="icon" aria-label="Move to trash" onClick={() => onTrash(item)}>
          <Trash2 aria-hidden="true" />
        </Button>
      )}
    </>
  );

  return (
    <div
      className="fixed inset-0 z-40 select-none text-white"
      role="dialog"
      aria-modal="true"
      aria-label={item.original_name}
      ref={dialogRef}
      onPointerMove={(e) => e.pointerType === "mouse" && showChrome()}
    >
      <motion.div className="absolute inset-0 bg-black" aria-hidden="true" style={{ opacity: bg }} />
      <div className="absolute inset-0 flex">
        <div className="relative flex min-w-0 flex-1 flex-col">
          <div className="relative min-h-0 flex-1" ref={boxRef}>
            <motion.div className="absolute inset-0" style={{ x: lx, y: ly, scale: ls, clipPath: clip, opacity: fade }}>
              {bgUrl && <div className="absolute inset-0 bg-cover bg-center opacity-25" style={{ backgroundImage: `url(${bgUrl})` }} aria-hidden="true" />}
              <div
                className={cn("viewer-stage", zoom.zoomed && "zoomed")}
                ref={stageRef}
                onPointerDown={(e) => {
                  lastPointer.current = e.pointerType;
                  drag.onPointerDown(e);
                  zoom.handlers.onPointerDown(e);
                }}
                onPointerMove={(e) => {
                  drag.onPointerMove(e);
                  zoom.handlers.onPointerMove(e);
                }}
                onPointerUp={(e) => {
                  drag.onPointerUp(e);
                  zoom.handlers.onPointerUp(e);
                }}
                onPointerCancel={(e) => {
                  drag.onPointerCancel(e);
                  zoom.handlers.onPointerCancel(e);
                }}
                onClick={onStageClick}
              >
                <motion.div className="absolute inset-0" style={{ x: trackX }}>
                  {slot(-1)}
                  {slot(1)}
                  {slot(0)}
                </motion.div>
                {failed[item.id] && <p className="absolute inset-x-6 top-1/2 m-0 -translate-y-1/2 text-center text-white/70">Could not load this photo.</p>}
                {rendering && (
                  <p className="absolute top-20 left-1/2 m-0 -translate-x-1/2 rounded-full bg-black/45 px-3.5 py-1.5 text-sm backdrop-blur-md" role="status">
                    Updating the picture…
                  </p>
                )}
                {showPending && !fallback && <p className="absolute inset-x-6 top-1/2 m-0 -translate-y-1/2 text-center text-white/70">This photo is still being processed.</p>}
                {(src ?? fallback) && (
                  <ZoomControls
                    zoomed={zoom.zoomed}
                    onIn={zoom.zoomIn}
                    onOut={zoom.zoomOut}
                    onFit={zoom.fit}
                    levelRef={levelRef}
                    announceRef={announceRef}
                    className="bottom-28 md:bottom-4"
                  />
                )}
              </div>
            </motion.div>

            <motion.div className="pointer-events-none absolute inset-0" style={{ opacity: chromeFade }} data-chrome>
              {/* Top: back, when and where, the actions (desktop) and the menu. */}
              <div
                className={cn(
                  "absolute inset-x-0 top-0 flex items-start gap-2 px-3 pt-[max(0.75rem,env(safe-area-inset-top))] pr-[max(0.75rem,env(safe-area-inset-right))] pl-[max(0.75rem,env(safe-area-inset-left))]",
                  barClass,
                )}
              >
                <Button variant="overlay" size="icon" className={auto} aria-label="Close" aria-keyshortcuts="Escape" onClick={() => void close()}>
                  <ChevronLeft aria-hidden="true" />
                </Button>
                <div className="min-w-0 flex-1 text-center md:text-left">
                  <div className="inline-flex max-w-full flex-col rounded-2xl bg-black/35 px-4 py-1 text-center backdrop-blur-md md:text-left">
                    <span className="truncate text-[0.9375rem] leading-snug font-semibold">
                      {validDate ? <time dateTime={item.sort_time}>{date.toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" })}</time> : item.original_name}
                    </span>
                    <span className="flex min-w-0 items-center justify-center gap-1.5 text-xs leading-snug text-white/75 md:justify-start">
                      <span className="truncate">{placeText || timeText || item.original_name}</span>
                      {edited && (
                        <>
                          <span aria-hidden="true">·</span>
                          <span className="shrink-0">Edited</span>
                        </>
                      )}
                      <span aria-hidden="true">·</span>
                      <span className="shrink-0 tabular-nums" aria-live="polite">
                        {position + 1} / {total}
                      </span>
                    </span>
                  </div>
                </div>
                <div className={cn("flex shrink-0 items-center gap-2", auto)}>
                  {desktop && actions}
                  {(
                    <Menu
                      label="More actions"
                      trigger={
                        <Button variant="overlay" size="icon" aria-label="More">
                          <Ellipsis aria-hidden="true" />
                        </Button>
                      }
                    >
                      {onAddToAlbum && (
                        <MenuItem onSelect={() => onAddToAlbum(item)}>
                          <FolderPlus aria-hidden="true" />
                          Add to album
                        </MenuItem>
                      )}
                      {slideshow && (
                        <MenuItem onSelect={playFromHere}>
                          <Play aria-hidden="true" />
                          Play from here
                        </MenuItem>
                      )}
                      {onSimilar && (
                        <MenuItem onSelect={() => onSimilar(item)}>
                          <Images aria-hidden="true" />
                          Similar photos
                        </MenuItem>
                      )}
                      {text.enabled && (
                        <MenuItem aria-keyshortcuts="t" onSelect={toggleText}>
                          <ScanText aria-hidden="true" />
                          {text.open ? "Hide text" : "Text in photo"}
                        </MenuItem>
                      )}
                      <MenuSeparator />
                      {files.length === 1 && !edited ? (
                        <MenuItem asChild>
                          <a href={files[0]!.href} rel="noopener" className="text-fg no-underline">
                            <Download aria-hidden="true" />
                            Download
                          </a>
                        </MenuItem>
                      ) : (
                        <>
                          <MenuLabel>Download</MenuLabel>
                          {files.map((f) => (
                            <MenuItem key={f.key} asChild>
                              <a href={f.href} rel="noopener" className="text-fg no-underline">
                                <Download aria-hidden="true" />
                                <span>Download {edited && !f.label ? "original" : f.label}</span>
                                <span className="ml-auto pl-3 text-sm text-muted">{fileSize(f.size)}</span>
                              </a>
                            </MenuItem>
                          ))}
                          {edited && (
                            <MenuItem disabled={preparing === item.id} onSelect={() => void downloadEdited()}>
                              <Download aria-hidden="true" />
                              {preparing === item.id ? "Preparing the edited photo…" : "Download edited (JPEG)"}
                            </MenuItem>
                          )}
                        </>
                      )}
                      {(onArchive || onHide) && <MenuSeparator />}
                      {onArchive && (
                        <MenuItem onSelect={() => onArchive(item)}>
                          {item.archived === true ? <ArchiveRestore aria-hidden="true" /> : <Archive aria-hidden="true" />}
                          {item.archived === true ? "Restore" : "Archive"}
                        </MenuItem>
                      )}
                      {onHide && (
                        <MenuItem onSelect={() => onHide(item)}>
                          {item.hidden === true ? <Eye aria-hidden="true" /> : <EyeOff aria-hidden="true" />}
                          {item.hidden === true ? "Unhide" : "Hide"}
                        </MenuItem>
                      )}
                    </Menu>
                  )}
                </div>
              </div>

              {(preparing === item.id || downloadError || tagError) && (
                <p
                  className={cn(
                    "absolute top-20 left-1/2 m-0 max-w-[calc(100%-2rem)] -translate-x-1/2 rounded-full bg-black/55 px-3.5 py-1.5 text-center text-sm backdrop-blur-md",
                    (downloadError || tagError) && "text-[#ffb4b4]",
                  )}
                  role={downloadError || tagError ? "alert" : "status"}
                >
                  {downloadError ?? tagError ?? "Preparing the edited photo…"}
                </p>
              )}

              {/* The sides: arrows for a mouse. */}
              <div className={cn("absolute inset-y-0 left-3 flex items-center pointer-coarse:hidden", barClass)}>
                <Button variant="overlay" size="icon" className={auto} aria-label="Previous photo" onClick={() => go(-1)} disabled={position === 0}>
                  <ChevronLeft aria-hidden="true" />
                </Button>
              </div>
              <div className={cn("absolute inset-y-0 right-3 flex items-center pointer-coarse:hidden", barClass)}>
                <Button variant="overlay" size="icon" className={auto} aria-label="Next photo" onClick={() => go(1)} disabled={position >= total - 1}>
                  <ChevronRight aria-hidden="true" />
                </Button>
              </div>

              {/* Bottom: the caption, the RAW + JPEG pair, the stack, the stars and (phone) the actions. */}
              <div
                className={cn(
                  "absolute inset-x-0 bottom-0 mx-auto flex w-full max-w-xl flex-col items-stretch gap-2 px-3 pb-[max(0.75rem,env(safe-area-inset-bottom))]",
                  barClass,
                )}
              >
                {detail?.caption && (
                  <p className="m-0 line-clamp-3 text-center text-[0.9375rem] whitespace-pre-wrap [text-shadow:0_1px_8px_rgb(0_0_0/0.7)] [overflow-wrap:anywhere]">{detail.caption}</p>
                )}
                {paired.length > 0 && (
                  <div className={cn("flex items-center gap-3 rounded-card bg-black/45 py-2 pr-2 pl-3.5 text-sm backdrop-blur-md", auto)}>
                    <div className="min-w-0 flex-1">
                      <strong className="font-semibold">{pairLabel(item, paired)}</strong>
                      <ul className="m-0 flex list-none flex-wrap gap-x-3 p-0 text-white/90 [overflow-wrap:anywhere]">
                        <li>
                          {item.original_name} <span className="text-white/60">{fileSize(item.size_bytes)}</span>
                        </li>
                        {paired.map((p) => (
                          <li key={p.id}>
                            {p.original_name} <span className="text-white/60">{fileSize(p.size_bytes)}</span>
                          </li>
                        ))}
                      </ul>
                    </div>
                    {onUnpair && (
                      <Button variant="overlay" size="sm" className="shrink-0 bg-white/15" onClick={() => onUnpair(item)}>
                        Unpair
                      </Button>
                    )}
                  </div>
                )}
                {stack && (
                  <div className={auto}>
                    <StackBar stack={stack} />
                  </div>
                )}
                {onRate && (
                  <div className="flex justify-center text-white/75 [--star:#f0c040]">
                    <div className={cn("rounded-full bg-black/35 px-1.5 backdrop-blur-md", auto)}>
                      <RatingStars value={item.rating} onChange={(n) => onRate(item, n)} label={`Rating of ${item.original_name}`} />
                    </div>
                  </div>
                )}
                {!desktop && <div className={cn("flex items-center justify-between gap-2 px-1", auto)}>{actions}</div>}
              </div>
            </motion.div>
          </div>
        </div>
        {loadDetails && (
          <DetailsPanel
            panelId="details-panel"
            open={info.open}
            state={infoState}
            onRetry={retryInfo}
            onEdit={setEditing}
            tags={detail?.tags}
            onTag={detail ? () => setTagging(true) : undefined}
            onUntag={(t) => void untag(t)}
            rating={item.rating}
            onRate={onRate ? (n) => onRate(item, n) : undefined}
            onClose={closeInfo}
            onLeave={info.leave}
            onNavigate={onClose}
            onEditPicture={canEditPicture ? () => setPicture({ cropping: false, turned: false }) : undefined}
            faces={peopleOn ? facesById[item.id] : undefined}
            dismissed={peopleOn ? dismissedById[item.id]?.length : undefined}
            onBringBack={peopleOn && dismissedById[item.id]?.length ? () => void bringBack(dismissedById[item.id]!) : undefined}
            faceBoxes={faceBoxes}
            onFaceBoxes={peopleOn ? setFaceBoxes : undefined}
            pets={
              petsOn && animalsById[item.id]?.length
                ? {
                    animals: animalsById[item.id]!,
                    picture: src,
                    onAdd: (animal) => setNamingAnimal(animal),
                    onNot: (animal) => void notThisPet(animal),
                    onKind: (animal, kind) => void animalIs(animal, kind),
                  }
                : undefined
            }
          />
        )}
        {text.enabled && (
          <TextPanel
            panelId="text-panel"
            open={text.open}
            state={text.state}
            onRetry={text.retry}
            onClose={closeText}
            showBoxes={text.boxes}
            onShowBoxes={text.setBoxes}
          />
        )}
      </div>
      {picture && editBase && (
        <RotateCrop
          key={`${item.id}-${detail?.recipe ? "edited" : "plain"}`}
          name={item.original_name}
          src={editBase}
          initial={fromServer(detail?.recipe)}
          fileSize={pixelSize}
          startCropping={picture.cropping}
          startTurned={picture.turned}
          onSave={savePicture}
          onRevert={revertPicture}
          onCancel={() => setPicture(null)}
        />
      )}
      {namingFace && (
        <AssignFaceDialog
          face={namingFace}
          previewUrl={src}
          onNotAFace={markedNotAFace}
          onCancel={() => setNamingFace(null)}
          onDone={() => {
            setNamingFace(null);
            setFacesVersion((v) => v + 1);
          }}
        />
      )}
      {namingAnimal && (
        <NamePetSheet
          detectionIds={[namingAnimal.id]}
          kind={namingAnimal.kind}
          onClose={() => setNamingAnimal(null)}
          onDone={(r) => {
            setNamingAnimal(null);
            setAnimalsVersion((v) => v + 1);
            const t = petNamedToast(r, (petId) =>
              void info.leave().then(() => {
                onClose();
                navigateRef.current?.(`/pets/${encodeURIComponent(petId)}`);
              }),
            );
            showToast(t);
          }}
        />
      )}
      {(peopleOn || petsOn) && <NavBridge navRef={navigateRef} />}
      {(peopleOn || petsOn) && toast}
      {tagging && (
        <TagDialog
          target={[item.id]}
          count={1}
          allowRemove={false}
          onClose={() => setTagging(false)}
          onDone={() => {
            setTagging(false);
            setEditVersion((v) => v + 1);
          }}
        />
      )}
      {editing && infoById[item.id] && (
        <EditDialog
          key={`${item.id}-${editing}`}
          kind={editing}
          target={[item.id]}
          count={1}
          current={{
            date: infoById[item.id]!.date,
            caption: infoById[item.id]!.caption,
            edits: infoById[item.id]!.edits,
            location: infoById[item.id]!.location
              ? { latitude: infoById[item.id]!.location!.latitude, longitude: infoById[item.id]!.location!.longitude }
              : null,
          }}
          onClose={() => setEditing(null)}
          onDone={() => {
            setEditing(null);
            setEditVersion((v) => v + 1);
            onEdited?.(item);
          }}
        />
      )}
    </div>
  );
}

/** Hands the router's `navigate` to the viewer (which may be drawn outside a router) once People is on. */
function NavBridge({ navRef }: { navRef: { current: ((path: string) => void) | null } }) {
  const navigate = useNavigate();
  useEffect(() => {
    navRef.current = (path) => void navigate(path);
    return () => {
      navRef.current = null;
    };
  }, [navigate, navRef]);
  return null;
}
