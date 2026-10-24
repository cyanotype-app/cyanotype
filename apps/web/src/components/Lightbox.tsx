import { Link } from "react-router-dom";
import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { api, downloadHref, editedHref, type AssetDetail, type AssetListItem, type PhotoDetails, type PhotoFace, type PhotoTag, type PhotoText } from "../api";
import { usePeopleEnabled } from "../usePeople";
import { AssignFaceDialog } from "./FaceChips";
import { useInfoHistory, useVerticalSwipe } from "../useInfoPanel";
import { useImageRect, useTextPanel } from "../useTextPanel";
import { TextBoxes, TextPanel } from "./TextPanel";
import { DetailsPanel, type DetailsState } from "./DetailsPanel";
import { EditDialog, type EditKind } from "./EditDialog";
import { placeLine } from "../map/geo";
import { fileSize, pairLabel, primaryFormat } from "../pairText";
import { placeholderUrl } from "../thumbhash";
import { HeartIcon } from "./Tile";
import { RatingStars } from "./Stars";
import { TagDialog } from "./TagDialog";
import { ratingKey } from "../ratingText";
import { fromServer, toRequest, type Recipe } from "../recipeText";
import { CropIcon, RotateCrop } from "./RotateCrop";

/** How often the viewer asks whether the worker has finished a picture or an edited download, and how many times. */
const POLL_MS = 1500;
const POLL_TRIES = 40;

interface Props {
  items: AssetListItem[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  loadDetail: (id: string) => Promise<AssetDetail>;
  /** Read the photo's details (the Info panel: date, place, camera, exposure, files). The Info button shows when given. */
  loadDetails?: (id: string) => Promise<PhotoDetails>;
  /** Read the text found in the photo (the Text panel: copy it, QR codes). Defaults to the server's; the Text button shows when `loadDetails` is given. */
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
  /** Archive the current photo, or restore it when it is archived (the button says which). Shown when given. */
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
}

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
}: Props) {
  const item = items[index];
  // Which photo's file chooser is open (it closes by itself when the photo changes).
  const [downloadsFor, setDownloadsFor] = useState<string | null>(null);
  const downloadsOpen = item !== undefined && downloadsFor === item.id;
  // Which photo's "More" menu is open (phones; it closes by itself when the photo changes, like the file chooser).
  const [moreFor, setMoreFor] = useState<string | null>(null);
  const moreOpen = item !== undefined && moreFor === item.id;
  const moreRef = useRef<HTMLDivElement>(null);
  const moreButton = useRef<HTMLButtonElement>(null);
  const moreMenu = useRef<HTMLDivElement>(null);
  const [details, setDetails] = useState<Record<string, AssetDetail>>({});
  const [failed, setFailed] = useState<Record<string, true>>({});
  const requested = useRef(new Set<string>());
  const dialogRef = useRef<HTMLDivElement>(null);
  const [imgLoaded, setImgLoaded] = useState<string | null>(null);

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
  // The people in the photo (when People is on), for the Info panel's chips; a face nobody has claimed can be named from there.
  const peopleOn = usePeopleEnabled() === true;
  const [facesById, setFacesById] = useState<Record<string, PhotoFace[]>>({});
  const [namingFace, setNamingFace] = useState<PhotoFace | null>(null);
  const [facesVersion, setFacesVersion] = useState(0);
  useEffect(() => {
    if (!peopleOn || !info.open || !loadDetails || !currentId) return;
    let live = true;
    const id = currentId;
    api.photoFaces(id).then(
      (r) => live && setFacesById((m) => ({ ...m, [id]: r.items })),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [peopleOn, info.open, loadDetails, currentId, facesVersion]);
  const swipe = useVerticalSwipe({ onUp: info.show, enabled: !!loadDetails && !info.open });
  const closeInfo = info.hide;
  // The Text panel: the Text button or `t`. It takes the Info panel's place (one panel at a time) and, like it, stays open while paging.
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
  const imgRef = useRef<HTMLImageElement>(null);
  const imageRect = useImageRect(stageRef, imgRef, [currentId, imgLoaded, text.open, text.boxes, info.open]);
  // The editor (date, location, caption) of the open photo, from the panel.
  const [editing, setEditing] = useState<EditKind | null>(null);
  const [editVersion, setEditVersion] = useState(0);
  // Adding a tag to the open photo (the dialog), and a failed removal.
  const [tagging, setTagging] = useState(false);
  const [tagError, setTagError] = useState<string | null>(null);
  // The rotate/crop editor of the open photo (the Crop button, or `e`, `r`, `c`), and the photo the worker is rendering after a save.
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
  // Closing (or leaving) with the panel open also pops its history entry.
  const { leave } = info;
  const close = useCallback(() => void leave().then(onClose), [leave, onClose]);
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

  const go = useCallback(
    (delta: number) => {
      const next = index + delta;
      if (next >= 0 && next < items.length) onIndexChange(next);
    },
    [index, items.length, onIndexChange],
  );

  // Keyboard: ←/→ navigate, Esc closes, Tab stays inside the dialog.
  // Rotate and crop: the editor works on the whole upright picture, which is the preview unless the photo is already cropped.
  const here = items[index];
  const hereDetail = here ? details[here.id] : undefined;
  const editBase = hereDetail ? (hereDetail.recipe ? (hereDetail.edit_base_url ?? null) : hereDetail.preview_url) : null;
  const canEditPicture = !!loadDetails && !!editBase && hereDetail?.rendering !== true;
  const pictureOpen = picture !== null;
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      // Typing in a dialog opened over the lightbox (add to album) is not a lightbox shortcut.
      if ((e.target as HTMLElement | null)?.closest("input, textarea, select")) return;
      // The rotate/crop editor owns the keyboard while it is open.
      if (pictureOpen) return;
      if (
        loadDetails &&
        ["e", "r", "c"].includes(e.key) &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.repeat &&
        [null, dialogRef.current].includes((e.target as HTMLElement | null)?.closest('[role="dialog"]') ?? null)
      ) {
        const cur = items[index];
        if (!cur || !canEditPicture) return;
        e.preventDefault();
        setPicture({ cropping: e.key === "c", turned: e.key === "r" });
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        // The More menu closes first (focus goes back to its button), then the panel; the next Escape closes the lightbox.
        if (moreOpen) {
          setMoreFor(null);
          moreButton.current?.focus();
        } else if (info.open) closeInfo();
        else if (text.open) closeText();
        else close();
      } else if (
        e.key === "i" &&
        loadDetails &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.repeat &&
        // Not while a dialog opened over the lightbox (add to album, share) owns the keyboard.
        [null, dialogRef.current].includes((e.target as HTMLElement | null)?.closest('[role="dialog"]') ?? null)
      ) {
        e.preventDefault();
        info.toggle();
      } else if (
        e.key === "t" &&
        text.enabled &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.repeat &&
        [null, dialogRef.current].includes((e.target as HTMLElement | null)?.closest('[role="dialog"]') ?? null)
      ) {
        e.preventDefault();
        if (!text.open) closeInfo();
        text.toggle();
      } else if (
        onRate &&
        ratingKey(e) !== null &&
        !e.repeat &&
        [null, dialogRef.current].includes((e.target as HTMLElement | null)?.closest('[role="dialog"]') ?? null)
      ) {
        const cur = items[index];
        if (!cur) return;
        e.preventDefault();
        onRate(cur, ratingKey(e)!);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      } else if ((e.key === "Delete" || e.key === "Backspace") && onTrash && !e.repeat) {
        const cur = items[index];
        if (!cur) return;
        e.preventDefault();
        onTrash(cur);
      } else if (e.key === "Tab" && dialogRef.current) {
        const f = dialogRef.current.querySelectorAll<HTMLElement>("button, a[href]");
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
  }, [go, close, onTrash, onRate, items, index, info, closeInfo, loadDetails, text, closeText, pictureOpen, canEditPicture, moreOpen]);

  // The More menu: focus its first item when it opens, and close it on a tap or click anywhere else.
  useEffect(() => {
    if (!moreOpen) return;
    moreMenu.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onDown = (e: Event) => {
      if (!moreRef.current?.contains(e.target as Node)) setMoreFor(null);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [moreOpen]);

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

  if (!item) return null;
  const detail = details[item.id];
  const src = detail?.preview_url ?? null;
  const fallback = detail?.thumb_url ?? item.thumb_url;
  const bg = placeholderUrl(item.thumbhash);
  const showPending = detail && !src;
  const date = new Date(item.sort_time);
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
  // The actions that sit in the More menu on a phone (and inline in the bar elsewhere; CSS decides which), in one list so
  // both places run the same handler.
  const overflow = [
    onAddToAlbum && { key: "album", label: "Add to album", run: onAddToAlbum },
    onArchive && { key: "archive", label: item.archived === true ? "Restore" : "Archive", run: onArchive },
    onHide && { key: "hide", label: item.hidden === true ? "Unhide" : "Hide", run: onHide },
    onTrash && { key: "trash", label: "Move to trash", run: onTrash },
  ].filter((a): a is { key: string; label: string; run: (item: AssetListItem) => void } => !!a);
  const inlineAction = (a: { key: string; label: string; run: (item: AssetListItem) => void }) => (
    <button key={a.key} type="button" className="btn btn-glass lightbox-overflow" onClick={() => a.run(item)}>
      {a.label}
    </button>
  );
  const toggleMore = () => {
    setDownloadsFor(null);
    setMoreFor(moreOpen ? null : item.id);
  };
  function onMenuKey(e: ReactKeyboardEvent<HTMLDivElement>) {
    const entries = Array.from(moreMenu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    const at = entries.indexOf(document.activeElement as HTMLElement);
    const to =
      e.key === "ArrowDown" ? (at + 1) % entries.length : e.key === "ArrowUp" ? (at <= 0 ? entries.length - 1 : at - 1) : e.key === "Home" ? 0 : e.key === "End" ? entries.length - 1 : -1;
    if (to >= 0) {
      e.preventDefault();
      entries[to]?.focus();
    } else if (e.key === "Tab") {
      setMoreFor(null);
      moreButton.current?.focus();
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

  return (
    <div className="lightbox" role="dialog" aria-modal="true" aria-label={item.original_name} ref={dialogRef}>
      <div className="lightbox-bar">
        <button type="button" className="btn btn-glass" onClick={close}>
          Close
        </button>
        <span className="lightbox-count" aria-live="polite">
          {position + 1} / {total}
        </span>
        <div className="lightbox-right">
          {loadDetails && (
            <button
              type="button"
              ref={infoButton}
              className={`btn btn-glass lightbox-info${info.open ? " on" : ""}`}
              aria-label="Info"
              aria-expanded={info.open}
              aria-controls="details-panel"
              aria-keyshortcuts="i"
              onClick={info.toggle}
            >
              <InfoIcon />
              <span className="lightbox-info-label">Info</span>
            </button>
          )}
          {text.enabled && (
            <button
              type="button"
              className={`btn btn-glass lightbox-text${text.open ? " on" : ""}`}
              aria-expanded={text.open}
              aria-controls="text-panel"
              aria-keyshortcuts="t"
              aria-label="Text"
              onClick={toggleText}
            >
              <TextIcon />
              <span className="lightbox-text-label">Text</span>
            </button>
          )}
          {onSimilar && (
            <button type="button" className="btn btn-glass lightbox-similar" aria-label="Similar photos" onClick={() => onSimilar(item)}>
              <SimilarIcon />
              <span className="lightbox-similar-label">Similar photos</span>
            </button>
          )}
          {onFavorite && (
            <button
              type="button"
              className={`btn btn-glass lightbox-heart${favorite ? " on" : ""}`}
              aria-pressed={favorite}
              aria-label={favorite ? "Remove from favorites" : "Add to favorites"}
              onClick={() => onFavorite(item, !favorite)}
            >
              <HeartIcon filled={favorite} />
            </button>
          )}
          {loadDetails && (
            <button
              type="button"
              className="btn btn-glass lightbox-crop"
              aria-label="Rotate and crop"
              aria-keyshortcuts="e"
              disabled={!canEditPicture}
              title={rendering ? "The picture is being updated" : "Rotate and crop"}
              onClick={() => setPicture({ cropping: false, turned: false })}
            >
              <CropIcon />
            </button>
          )}
          <div className="lightbox-actions">
            {files.length === 1 && !edited ? (
              <a className="btn btn-glass" href={files[0]!.href} rel="noopener">
                Download
              </a>
            ) : (
              <button
                type="button"
                className="btn btn-glass"
                aria-expanded={downloadsOpen}
                aria-controls="lightbox-downloads"
                onClick={() => {
                  setMoreFor(null);
                  setDownloadsFor(downloadsOpen ? null : item.id);
                }}
              >
                Download
              </button>
            )}
            {overflow.filter((a) => a.key === "album").map(inlineAction)}
            {onShare && (
              <button type="button" className="btn btn-glass" onClick={() => onShare(item)}>
                Share
              </button>
            )}
            {overflow.filter((a) => a.key !== "album").map(inlineAction)}
            {overflow.length > 0 && (
              <div className="lightbox-more" ref={moreRef}>
                <button
                  type="button"
                  ref={moreButton}
                  className="btn btn-glass"
                  aria-haspopup="menu"
                  aria-expanded={moreOpen}
                  aria-controls="lightbox-more-menu"
                  onClick={toggleMore}
                >
                  More
                </button>
                {moreOpen && (
                  <div className="lightbox-menu" id="lightbox-more-menu" role="menu" aria-label="More actions" ref={moreMenu} onKeyDown={onMenuKey}>
                    {overflow.map((a) => (
                      <button
                        key={a.key}
                        type="button"
                        role="menuitem"
                        className="lightbox-menu-item"
                        onClick={() => {
                          setMoreFor(null);
                          moreButton.current?.focus();
                          a.run(item);
                        }}
                      >
                        {a.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
      {(files.length > 1 || edited) && downloadsOpen && (
        <div className="lightbox-downloads" id="lightbox-downloads">
          {files.map((f) => (
            <a key={f.key} className="btn btn-glass" href={f.href} rel="noopener">
              Download {edited && !f.label ? "original" : f.label} <span className="muted">{fileSize(f.size)}</span>
            </a>
          ))}
          {edited && (
            <button type="button" className="btn btn-glass" disabled={preparing === item.id} onClick={() => void downloadEdited()}>
              {preparing === item.id ? "Preparing the edited photo…" : "Download edited (JPEG)"}
            </button>
          )}
        </div>
      )}
      {downloadError && (
        <p className="lightbox-msg lightbox-msg-top" role="alert">
          {downloadError}
        </p>
      )}
      <div className={`lightbox-body${info.open || text.open ? " with-info" : ""}`}>
      <div className="lightbox-main">
      <div className="lightbox-stage" ref={stageRef} {...swipe} style={bg ? { backgroundImage: `url(${bg})` } : undefined}>
        <button
          type="button"
          className="nav nav-prev"
          aria-label="Previous photo"
          onClick={() => go(-1)}
          disabled={position === 0}
        >
          ‹
        </button>
        {(src ?? fallback) && (
          <img
            key={item.id}
            ref={imgRef}
            src={src ?? fallback ?? undefined}
            alt={item.original_name}
            className={src && imgLoaded === item.id ? "loaded" : src ? "loading" : "loaded"}
            onLoad={() => setImgLoaded(item.id)}
          />
        )}
        {text.open && text.boxes && text.current && imageRect && <TextBoxes text={text.current} style={imageRect} />}
        {failed[item.id] && <p className="lightbox-msg">Could not load this photo.</p>}
        {rendering && (
          <p className="lightbox-updating" role="status">
            Updating the picture…
          </p>
        )}
        {showPending && !fallback && <p className="lightbox-msg">This photo is still being processed.</p>}
        <button
          type="button"
          className="nav nav-next"
          aria-label="Next photo"
          onClick={() => go(1)}
          disabled={position >= total - 1}
        >
          ›
        </button>
      </div>
      {onRate && (
        <div className="lightbox-rating">
          <RatingStars value={item.rating} onChange={(n) => onRate(item, n)} label={`Rating of ${item.original_name}`} />
        </div>
      )}
      {detail?.caption && <p className="lightbox-words">{detail.caption}</p>}
      <p className="lightbox-caption">
        <strong>{item.original_name}</strong>
        <span className="muted">
          {item.width && item.height ? `${item.width} × ${item.height} · ` : ""}
          {edited && <span className="lightbox-edited">edited · </span>}
          <time dateTime={item.sort_time}>{Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString()}</time>
          {detail?.place && <span className="lightbox-place"> · {placeLine(detail.place)}</span>}
        </span>
      </p>
      {paired.length > 0 && !info.open && (
        <div className="lightbox-pair">
          <strong>{pairLabel(item, paired)}</strong>
          <ul>
            <li>
              {item.original_name} <span className="muted">{fileSize(item.size_bytes)}</span>
            </li>
            {paired.map((p) => (
              <li key={p.id}>
                {p.original_name} <span className="muted">{fileSize(p.size_bytes)}</span>
              </li>
            ))}
          </ul>
          {onUnpair && (
            <button type="button" className="btn btn-glass" onClick={() => onUnpair(item)}>
              Unpair
            </button>
          )}
        </div>
      )}
      {detail?.albums && detail.albums.length > 0 && !info.open && (
        <p className="lightbox-albums">
          <span className="muted">In albums:</span>{" "}
          {detail.albums.map((a, i) => (
            <span key={a.id}>
              {i > 0 && ", "}
              <Link to={`/albums/${a.id}`} onClick={onClose}>
                {a.title}
              </Link>
            </span>
          ))}
        </p>
      )}
      </div>
      {text.open && text.enabled && (
        <TextPanel
          panelId="text-panel"
          state={text.state}
          onRetry={text.retry}
          onClose={closeText}
          showBoxes={text.boxes}
          onShowBoxes={text.setBoxes}
        />
      )}
      {info.open && loadDetails && (
        <DetailsPanel
          panelId="details-panel"
          state={infoState}
          onRetry={retryInfo}
          onEdit={setEditing}
          tags={detail?.tags}
          onTag={detail ? () => setTagging(true) : undefined}
          onUntag={(t) => void untag(t)}
          rating={item.rating}
          onRate={onRate ? (n) => onRate(item, n) : undefined}
          onClose={closeInfo}
          onLeave={leave}
          onNavigate={onClose}
          onEditPicture={canEditPicture ? () => setPicture({ cropping: false, turned: false }) : undefined}
          faces={peopleOn ? facesById[item.id] : undefined}
          onNameFace={setNamingFace}
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
      {tagError && (
        <p className="lightbox-msg" role="alert">
          {tagError}
        </p>
      )}
      {namingFace && (
        <AssignFaceDialog
          face={namingFace}
          onCancel={() => setNamingFace(null)}
          onDone={() => {
            setNamingFace(null);
            setFacesVersion((v) => v + 1);
          }}
        />
      )}
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

/** A circled "i". */
function InfoIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5.5" />
      <circle cx="12" cy="7.8" r="0.6" fill="currentColor" />
    </svg>
  );
}

/** Lines of text. On a phone this is all the button shows. */
function TextIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
      <path d="M4 6h16M4 12h16M4 18h10" />
    </svg>
  );
}

/** Two overlapping frames. On a phone this is all the button shows (the bar has no room for its words). */
function SimilarIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="7" width="13" height="13" rx="2" />
      <path d="M8 3h11a2 2 0 0 1 2 2v11" strokeLinecap="round" />
    </svg>
  );
}
