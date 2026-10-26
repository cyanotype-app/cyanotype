import { CheckSquare, ImagePlus, Images, MoreHorizontal, Pencil, Play, Share2, SlidersHorizontal, Sparkles, Trash2 } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useState, type FormEvent } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { api, ApiError, isScope, type Album as AlbumData, type AssetListItem, type SmartRules } from "../api";
import { useAuth } from "../auth";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { Enter, Empty, InlineError, Loading, PageProblem, useTitleCut } from "../components/pageKit";
import { SelectionBar } from "../components/SelectionBar";
import { RateTools } from "../components/RateTools";
import { ZipButton } from "../components/ZipButton";
import { TagTools } from "../components/TagTools";
import { Field, FormError, Modal, TextInput } from "../components/sheetForm";
import { SmartAlbumEditor } from "../components/SmartAlbumEditor";
import { describeRules, smartCount } from "../smartText";
import { ShareDialog, type ShareTarget } from "../components/ShareDialog";
import { useToast } from "../components/Toast";
import { similarUrl } from "../searchText";
import { dateRange, hiddenNote, photoCount } from "../albumText";
import { useSelection } from "../selection";
import { PageHeader } from "../shell/PageHeader";
import { useAssetPages } from "../useAssetPages";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { scopeSource } from "../slideshow/sources";
import { placeholderUrl } from "../thumbhash";
import { Button, buttonVariants } from "../ui/Button";
import { Menu, MenuItem, MenuSeparator } from "../ui/Menu";

const PAGE = 60;

export function AlbumPage() {
  const { id = "" } = useParams();
  return <AlbumView key={id} id={id} />;
}

/** One album: a cover hero, its photos, selection actions, "Add photos", sharing, and rename / cover / rules / delete in the menu. */
function AlbumView({ id }: { id: string }) {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const slideshow = useSlideshow();
  const [album, setAlbum] = useState<AlbumData | null>(null);
  const [problem, setProblem] = useState<{ status: number; message: string } | null>(null);
  const [editing, setEditing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [adding, setAdding] = useState(false);
  const [editingRules, setEditingRules] = useState(false);
  const [ruleError, setRuleError] = useState<string | null>(null);
  const [sharing, setSharing] = useState<ShareTarget | null>(null);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState<number | null>(null);
  const { toast, show } = useToast();

  const fetchPage = useCallback(
    (cursor: string | null) => api.listAlbumAssets(id, { cursor, limit: PAGE }),
    [id],
  );
  const { items, setItems, done, started, error: pagesError, loadMore, sentinel, refreshItem, reset } =
    useAssetPages(fetchPage);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, album?.photo_count);
  const titleCut = useTitleCut(album?.title ?? "");

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  // Hearts: shown at once, taken back if the server refuses.
  const setFavorite = useCallback(
    async (target: string, favorite: boolean) => {
      refreshItem(target, { favorite });
      try {
        await api.favoriteAssets([target], favorite);
      } catch (e) {
        refreshItem(target, { favorite: !favorite });
        failure(e, favorite ? "Couldn’t add to favorites." : "Couldn’t remove from favorites.");
      }
    },
    [refreshItem, failure],
  );

  const rateOne = useCallback(
    async (it: { id: string; rating?: number }, rating: number) => {
      const was = it.rating ?? 0;
      refreshItem(it.id, { rating });
      try {
        await api.rateAssets([it.id], rating);
      } catch (e) {
        refreshItem(it.id, { rating: was });
        failure(e, "Couldn’t rate.");
      }
    },
    [refreshItem, failure],
  );

  const loadAlbum = useCallback(async () => {
    try {
      setAlbum(await api.getAlbum(id));
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setProblem({ status: e instanceof ApiError ? e.status : 0, message: e instanceof Error ? e.message : "Could not load the album." });
    }
  }, [id, setAnonymous]);
  useEffect(() => {
    void loadAlbum();
  }, [loadAlbum]);

  async function save(patch: { title: string; description: string | null }) {
    setBusy(true);
    try {
      setAlbum(await api.updateAlbum(id, patch));
      setEditing(false);
    } catch (e) {
      failure(e, "Couldn’t save.");
    } finally {
      setBusy(false);
    }
  }

  async function saveRules(title: string, rules: SmartRules) {
    setBusy(true);
    setRuleError(null);
    try {
      setAlbum(await api.updateAlbum(id, { title, rules }));
      setEditingRules(false);
      reset();
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setRuleError(e instanceof Error ? e.message : "Could not save the rules.");
    } finally {
      setBusy(false);
    }
  }

  async function removeSelected() {
    const count = sel.count;
    if (count === 0) return;
    const target = sel.target({ album: id });
    setBusy(true);
    try {
      await api.removeFromAlbum(id, target);
      if (isScope(target)) {
        // Photos that were never loaded went too: start the list over.
        setOpen(null);
        reset();
      } else {
        const gone = new Set(target);
        if (items.every((i) => gone.has(i.id))) setOpen(null);
        setItems((cur) => cur.filter((i) => !gone.has(i.id)));
      }
      sel.setSelecting(false);
      show({ message: `Removed ${count} from the album` });
      void loadAlbum();
    } catch (e) {
      failure(e, "Couldn’t remove them.");
    } finally {
      setBusy(false);
    }
  }

  async function setCover() {
    const [cover] = sel.ids;
    if (sel.count !== 1 || sel.all || !cover) return;
    setBusy(true);
    try {
      setAlbum(await api.updateAlbum(id, { cover_asset_id: cover }));
      sel.setSelecting(false);
      show({ message: "Cover updated" });
    } catch (e) {
      failure(e, "Couldn’t set the cover.");
    } finally {
      setBusy(false);
    }
  }

  // Separate a RAW + JPEG pair: the photo loses its RAW badge (the RAW becomes a photo of its own in the library).
  const unpairItem = useCallback(
    async (it: AssetListItem) => {
      try {
        await api.unpair(it.id);
      } catch (e) {
        failure(e, "Couldn’t unpair.");
        return;
      }
      refreshItem(it.id, { paired: [] });
      show({ message: "Unpaired. They show as separate photos now." });
    },
    [failure, show, refreshItem],
  );

  async function deleteAlbum() {
    setBusy(true);
    try {
      await api.deleteAlbum(id);
      navigate("/albums", { replace: true });
    } catch (e) {
      setConfirmDelete(false);
      setBusy(false);
      failure(e, "Couldn’t delete the album.");
    }
  }

  const onPhotosAdded = useCallback(
    async (count: number) => {
      setAdding(false);
      if (count > 0) {
        show({ message: `Added ${count} to the album` });
        // Newly added photos belong anywhere in the timeline: start the list over.
        reset();
        void loadAlbum();
      } else {
        show({ message: "Those photos are already in the album" });
      }
    },
    [show, reset, loadAlbum],
  );

  // Keyboard: Escape clears the selection, then leaves selection mode.
  useEffect(() => {
    if (open !== null || confirmDelete || adding || editing || editingRules || sharing) return;
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "Escape" && sel.selecting) {
        e.preventDefault();
        sel.escape();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, confirmDelete, adding, editing, editingRules, sharing, sel]);

  const select = useCallback(
    (i: number) => {
      setOpen(i);
      if (i >= items.length - 3 && !done && !pagesError) void loadMore();
    },
    [items.length, done, pagesError, loadMore],
  );
  const closeLightbox = useCallback(() => setOpen(null), []);

  if (problem) {
    return (
      <PageProblem
        title={problem.status === 404 ? "Album not found" : "Couldn’t load the album"}
        message={problem.status === 404 ? "It may have been deleted." : problem.message}
        back={{ to: "/albums", label: "Albums" }}
      />
    );
  }
  if (!album || !started) {
    return (
      <>
        <PageHeader title={album?.title ?? "Album"} back={{ to: "/albums", label: "Albums" }} />
        <Loading>Loading the album…</Loading>
        {toast}
      </>
    );
  }

  const facts = [
    album.smart ? smartCount(album.photo_count, album.photo_count_capped) : photoCount(album.photo_count),
    hiddenNote(album.hidden_count),
    dateRange(album.first_at, album.last_at),
  ]
    .filter(Boolean)
    .join(" · ");
  const pill = buttonVariants({ variant: "secondary", size: "sm" });

  return (
    <>
      <PageHeader
        title={album.title}
        back={{ to: "/albums", label: "Albums" }}
        actions={
          <>
            <Button size="icon" variant="ghost" aria-label="Share" onClick={() => setSharing({ kind: "album", albumId: id, albumTitle: album.title })}>
              <Share2 />
            </Button>
            {!album.smart && (
              <Button size="icon" variant="ghost" aria-label="Add photos" onClick={() => setAdding(true)}>
                <ImagePlus />
              </Button>
            )}
            <Menu
              label="Album"
              trigger={
                <Button size="icon" variant="ghost" aria-label="More">
                  <MoreHorizontal />
                </Button>
              }
            >
              {album.smart ? (
                <MenuItem onSelect={() => setEditingRules(true)}>
                  <SlidersHorizontal />
                  Edit rules
                </MenuItem>
              ) : (
                <>
                  <MenuItem onSelect={() => setEditing(true)}>
                    <Pencil />
                    Edit details
                  </MenuItem>
                  {items.length > 0 && (
                    <MenuItem
                      onSelect={() => {
                        sel.setSelecting(true);
                        show({ message: "Pick a photo, then choose Set as cover." });
                      }}
                    >
                      <Images />
                      Choose cover
                    </MenuItem>
                  )}
                </>
              )}
              <MenuSeparator />
              <MenuItem danger onSelect={() => setConfirmDelete(true)}>
                <Trash2 />
                Delete album
              </MenuItem>
            </Menu>
          </>
        }
      />
      <Enter className="mx-auto max-w-[96rem] px-4 md:px-8">
        {album.cover?.thumb_url && <Hero thumbhash={album.cover.thumbhash} url={album.cover.thumb_url} />}
        <div className="flex flex-col gap-1.5 pt-3.5 pb-1">
          {titleCut && <p className="text-[1.25rem] leading-snug font-semibold tracking-tight text-pretty [overflow-wrap:anywhere]">{album.title}</p>}
          <p className="flex items-center gap-2 text-[0.9375rem] text-muted">
            {album.smart && (
              <span className="inline-flex items-center gap-1 rounded-full bg-surface px-2 py-0.5 text-xs font-medium text-fg">
                <Sparkles className="size-3" aria-hidden="true" />
                Smart
              </span>
            )}
            {facts}
          </p>
          {album.description && <p className="max-w-prose text-[0.9375rem] text-pretty [overflow-wrap:anywhere]">{album.description}</p>}
          {album.smart && album.rules && (
            <p className="max-w-prose text-sm text-muted [overflow-wrap:anywhere]">
              <strong className="font-semibold text-fg">Rules:</strong> {describeRules(album.rules)}
            </p>
          )}
          {album.smart && <p className="max-w-prose text-sm text-muted">Smart albums update by themselves: they show whatever matches the rules when you open them.</p>}
        </div>
        {items.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 py-3">
            {slideshow && album.photo_count > 0 && (
              <Button variant="secondary" size="sm" onClick={() => slideshow.start(scopeSource({ album: id }, album.title))}>
                <Play />
                Slideshow
              </Button>
            )}
            <ZipButton
              className={pill}
              target={{ scope: { album: id }, except: [] }}
              name={album.title}
              onStarted={(message) => show({ message })}
              onError={(e) => failure(e, "Couldn’t download.")}
            />
            <Button variant="secondary" size="sm" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
              <CheckSquare />
              Select
            </Button>
          </div>
        )}
      </Enter>
      {sel.selecting && (
        <div className="mx-auto max-w-[96rem] px-4 md:px-8">
          <SelectionBar
            count={sel.count}
            total={album.photo_count}
            onSelectAll={sel.selectAll}
            onClear={sel.clear}
            onDone={() => sel.setSelecting(false)}
          >
            {!album.smart && (
              <Button variant="quiet" size="sm" disabled={sel.count !== 1 || sel.all || busy} onClick={() => void setCover()}>
                Set as cover
              </Button>
            )}
            <ZipButton
              className={buttonVariants({ variant: "quiet", size: "sm" })}
              target={sel.target({ album: id })}
              name={album.title}
              disabled={sel.count === 0}
              onStarted={(message) => show({ message })}
              onError={(e) => failure(e, "Couldn’t download.")}
            />
            <RateTools
              target={sel.target({ album: id })}
              count={sel.count}
              disabled={sel.count === 0 || busy}
              onDone={(rating, _n, message) => {
                const picked = sel.all ? null : new Set(sel.ids);
                sel.setSelecting(false);
                if (picked) for (const pid of picked) refreshItem(pid, { rating });
                else reset();
                show({ message });
              }}
              onError={(e) => failure(e, "Couldn’t rate.")}
            />
            <TagTools
              target={sel.target({ album: id })}
              count={sel.count}
              disabled={sel.count === 0 || busy}
              onDone={(message) => {
                sel.setSelecting(false);
                show({ message });
              }}
            />
            <Button variant="quiet" size="sm" disabled={sel.ids.length === 0 || busy} onClick={() => setSharing({ kind: "photos", ids: sel.ids })}>
              Share
            </Button>
            {!album.smart && (
              <Button variant="primary" size="sm" disabled={sel.count === 0 || busy} onClick={() => void removeSelected()}>
                Remove from album
              </Button>
            )}
          </SelectionBar>
        </div>
      )}
      {items.length === 0 ? (
        <Empty
          icon={Images}
          title={album.smart ? "No photo matches these rules" : "This album is empty"}
          action={
            !album.smart && (
              <Button variant="primary" onClick={() => setAdding(true)}>
                <ImagePlus />
                Add photos
              </Button>
            )
          }
        >
          {album.smart ? "It fills by itself when photos match." : "Add photos from your library."}
        </Empty>
      ) : (
        <div className="md:px-8">
          <Grid
            items={items}
            onOpen={select}
            onThumbError={(i) => void api.getAsset(i).then((d) => refreshItem(i, { thumb_url: d.thumb_url, thumbhash: d.thumbhash }), () => undefined)}
            selecting={sel.selecting}
            selected={sel.selected}
            onSelect={sel.select}
            onLongPress={sel.begin}
            onFavorite={(target, favorite) => void setFavorite(target, favorite)}
          />
        </div>
      )}
      <div ref={sentinel} className="h-px" aria-hidden="true" />
      {pagesError && <InlineError onRetry={() => void loadMore()}>{pagesError}</InlineError>}
      {open !== null && items.length > 0 && (
        <Lightbox
          items={items}
          index={Math.min(open, items.length - 1)}
          onIndexChange={select}
          onClose={closeLightbox}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onShare={(it) => setSharing({ kind: "photos", ids: [it.id] })}
          onFavorite={(it, favorite) => void setFavorite(it.id, favorite)}
          onRate={(it, n) => void rateOne(it, n)}
          onUnpair={(it) => void unpairItem(it)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
      {confirmDelete && (
        <ConfirmSheet
          title="Delete this album?"
          confirmLabel="Delete album"
          danger
          busy={busy}
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void deleteAlbum()}
        >
          <p>The photos stay in your library. Only the album is deleted.</p>
        </ConfirmSheet>
      )}
      {sharing && <ShareDialog target={sharing} onClose={() => setSharing(null)} />}
      {editing && <AlbumForm album={album} busy={busy} onSave={(p) => void save(p)} onCancel={() => setEditing(false)} />}
      {editingRules && album.rules && (
        <SmartAlbumEditor
          title="Edit rules"
          submitLabel="Save"
          initialTitle={album.title}
          initialRules={album.rules}
          busy={busy}
          error={ruleError}
          onSubmit={(t, r) => void saveRules(t, r)}
          onCancel={() => {
            setEditingRules(false);
            setRuleError(null);
          }}
        />
      )}
      {adding && <PhotoPicker albumId={id} onCancel={() => setAdding(false)} onAdded={(n) => void onPhotosAdded(n)} />}
      {toast}
    </>
  );
}

/** The cover, wide: its soft thumbnail over the ThumbHash it came from (an album with no cover has no hero). */
function Hero({ thumbhash, url }: { thumbhash: string | null; url: string }) {
  const bg = placeholderUrl(thumbhash);
  return (
    <div
      className="relative mt-1 h-44 overflow-hidden rounded-sheet bg-surface bg-cover md:h-60"
      style={bg ? { backgroundImage: `url(${bg})` } : undefined}
    >
      <img src={url} alt="" decoding="async" draggable={false} className="size-full scale-[1.03] object-cover blur-[1.5px]" />
    </div>
  );
}

function AlbumForm({
  album,
  busy,
  onSave,
  onCancel,
}: {
  album: AlbumData;
  busy: boolean;
  onSave: (p: { title: string; description: string | null }) => void;
  onCancel: () => void;
}) {
  const id = useId();
  const [title, setTitle] = useState(album.title);
  const [description, setDescription] = useState(album.description ?? "");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (title.trim()) onSave({ title: title.trim(), description: description.trim() || null });
  };
  return (
    <Modal
      onClose={onCancel}
      title="Edit details"
      size="md"
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id} disabled={busy || !title.trim()}>
            Save
          </Button>
        </>
      }
    >
      <form id={id} className="flex flex-col gap-4" onSubmit={submit}>
        <Field label="Title" htmlFor={`${id}-title`}>
          <TextInput id={`${id}-title`} data-autofocus="" value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} />
        </Field>
        <Field label="Description" htmlFor={`${id}-desc`}>
          <textarea
            id={`${id}-desc`}
            className="block min-h-24 w-full resize-y rounded-card bg-surface px-3.5 py-3 text-base text-fg ring-1 ring-line outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-focus md:text-[0.9375rem]"
            rows={3}
            placeholder="Optional"
            value={description}
            maxLength={5000}
            onChange={(e) => setDescription(e.target.value)}
          />
        </Field>
      </form>
    </Modal>
  );
}

/** A sheet over the whole library: select photos (same grid) and add them. */
function PhotoPicker({
  albumId,
  onCancel,
  onAdded,
}: {
  albumId: string;
  onCancel: () => void;
  onAdded: (count: number) => void;
}) {
  const { setAnonymous } = useAuth();
  const fetchPage = useCallback((cursor: string | null) => api.listAssets({ cursor, limit: PAGE, stacks: "expanded" }), []);
  const {
    items: pickItems,
    started: pickStarted,
    error: pickError,
    loadMore: pickMore,
    sentinel: pickSentinel,
  } = useAssetPages(fetchPage);
  const itemIds = useMemo(() => pickItems.map((i) => i.id), [pickItems]);
  const [libraryTotal, setLibraryTotal] = useState<number | undefined>(undefined);
  const sel = useSelection(itemIds, libraryTotal);
  const { setSelecting } = sel;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setSelecting(true), [setSelecting]);
  useEffect(() => {
    // "Select all" in the picker means the whole library (every photo of a stack, not its tile); the count is the timeline's total.
    let live = true;
    api.timeline({ stacks: "expanded" }).then(
      (t) => live && setLibraryTotal(t.total),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, []);

  async function add() {
    setBusy(true);
    setError(null);
    try {
      onAdded((await api.addToAlbum(albumId, sel.target("library"))).count);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not add the photos.");
      setBusy(false);
    }
  }

  const total = libraryTotal ?? pickItems.length;
  return (
    <Modal
      onClose={onCancel}
      title="Add photos"
      description={<span aria-live="polite">{sel.count === 0 ? "Pick the photos to add." : `${sel.count.toLocaleString("en-US")} selected`}</span>}
      size="lg"
      className="h-[92dvh] md:h-[86dvh] md:max-w-5xl"
      footer={
        <div className="flex w-full items-center gap-2">
          <Button variant="quiet" size="sm" onClick={sel.selectAll} disabled={total === 0 || sel.count === total}>
            Select all
          </Button>
          <Button variant="quiet" size="sm" onClick={sel.clear} disabled={sel.count === 0}>
            Clear
          </Button>
          <Button variant="primary" className="ml-auto" disabled={sel.count === 0 || busy} onClick={() => void add()}>
            {sel.count === 0 ? "Add" : `Add ${sel.count.toLocaleString()}`}
          </Button>
        </div>
      }
    >
      {error && <FormError className="pb-2">{error}</FormError>}
      {pickStarted && pickItems.length === 0 && <p className="py-10 text-center text-sm text-muted">Your library is empty.</p>}
      <div className="-mx-5 -mb-5">
        <Grid
          items={pickItems}
          onOpen={() => undefined}
          onThumbError={() => undefined}
          selecting
          selected={sel.selected}
          onSelect={sel.select}
        />
      </div>
      <div ref={pickSentinel} className="h-px" aria-hidden="true" />
      {pickError && <InlineError onRetry={() => void pickMore()}>{pickError}</InlineError>}
    </Modal>
  );
}
