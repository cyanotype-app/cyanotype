import { CheckSquare, Images, Merge, MoreHorizontal, Pencil, Play, Search, Trash2 } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, type Tag } from "../api";
import { useAuth } from "../auth";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { NameDialog } from "../components/NameDialog";
import { Empty, InlineError, Loading, PageProblem, useTitleCut } from "../components/pageKit";
import { RateTools } from "../components/RateTools";
import { ZipButton } from "../components/ZipButton";
import { SelectionBar } from "../components/SelectionBar";
import { FormError, inputClass, Modal, Note } from "../components/sheetForm";
import { TagTools } from "../components/TagTools";
import { useToast } from "../components/Toast";
import { useSelection } from "../selection";
import { similarUrl } from "../searchText";
import { PageHeader } from "../shell/PageHeader";
import { tagPhotos, tagSearchHref } from "../tagText";
import { useAssetPages } from "../useAssetPages";
import { Button, buttonVariants } from "../ui/Button";
import { Menu, MenuItem, MenuSeparator } from "../ui/Menu";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { scopeSource } from "../slideshow/sources";

const PAGE = 60;

export function TagPage() {
  const { id = "" } = useParams();
  return <TagView key={id} id={id} />;
}

/** One tag: its photos (a filter search for the tag), and Rename, Merge into another tag and Delete. */
function TagView({ id }: { id: string }) {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const { toast, show } = useToast();
  const [tags, setTags] = useState<Tag[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"rename" | "merge" | "delete" | null>(null);
  const [busy, setBusy] = useState(false);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      setTags((await api.listTags({ limit: 5000 })).items);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setProblem(e instanceof Error ? e.message : "Could not load the tag.");
    }
  }, [setAnonymous]);
  useEffect(() => {
    void load();
  }, [load]);
  const tag = tags?.find((t) => t.id === id) ?? null;
  const titleCut = useTitleCut(tag?.name ?? "");

  const fetchPage = useCallback(
    async (cursor: string | null) => {
      if (!tag) return { items: [], next_cursor: null, next_before: null };
      const r = await api.search({ tags: tag.name }, { cursor, limit: PAGE });
      return { items: r.items, next_cursor: r.next_cursor, next_before: null };
    },
    [tag],
  );
  if (problem) {
    return (
      <PageProblem
        title="Couldn’t load the tag"
        message={problem}
        back={{ to: "/tags", label: "Tags" }}
        action={
          <Button asChild variant="primary">
            <Link to="/tags">All tags</Link>
          </Button>
        }
      />
    );
  }
  if (tags === null) {
    return (
      <>
        <PageHeader title="Tag" back={{ to: "/tags", label: "Tags" }} />
        <Loading>Loading the tag…</Loading>
      </>
    );
  }
  if (!tag) {
    return (
      <PageProblem
        title="Tag not found"
        message="It may have been renamed into another tag or deleted."
        back={{ to: "/tags", label: "Tags" }}
        action={
          <Button asChild variant="primary">
            <Link to="/tags">All tags</Link>
          </Button>
        }
      />
    );
  }

  async function rename(name: string) {
    setBusy(true);
    setDialogError(null);
    try {
      await api.renameTag(id, name);
      setDialog(null);
      await load();
      show({ message: `Renamed to ${name}` });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else if (e instanceof ApiError && e.code === "tag_exists") {
        const target = typeof e.details?.existing_id === "string" ? e.details.existing_id : null;
        setDialogError(`You already have a tag called “${name}”.${target ? " Use Merge to combine them." : ""}`);
      } else setDialogError(e instanceof Error ? e.message : "Could not rename the tag.");
    } finally {
      setBusy(false);
    }
  }
  async function merge(into: Tag) {
    setBusy(true);
    setDialogError(null);
    try {
      const r = await api.mergeTag(id, into.id);
      show({ message: `Merged into ${into.name} (${tagPhotos(r.photos)})` });
      navigate(`/tags/${into.id}`, { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setDialogError(e instanceof Error ? e.message : "Could not merge the tags.");
      setBusy(false);
    }
  }
  async function remove() {
    setBusy(true);
    try {
      await api.deleteTag(id);
      navigate("/tags", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setDialogError(e instanceof Error ? e.message : "Could not delete the tag.");
      setBusy(false);
    }
  }

  return (
    <>
      <PageHeader
        title={tag.name}
        subtitle={tagPhotos(tag.count)}
        back={{ to: "/tags", label: "Tags" }}
        actions={
          <Menu
            label="Tag"
            trigger={
              <Button size="icon" variant="ghost" aria-label="More">
                <MoreHorizontal />
              </Button>
            }
          >
            <MenuItem
              onSelect={() => {
                setDialogError(null);
                setDialog("rename");
              }}
            >
              <Pencil />
              Rename
            </MenuItem>
            <MenuItem
              disabled={tags.length < 2}
              onSelect={() => {
                setDialogError(null);
                setDialog("merge");
              }}
            >
              <Merge />
              Merge into…
            </MenuItem>
            <MenuItem asChild>
              <Link to={tagSearchHref(tag.name)}>
                <Search />
                Search with filters
              </Link>
            </MenuItem>
            <MenuSeparator />
            <MenuItem
              danger
              onSelect={() => {
                setDialogError(null);
                setDialog("delete");
              }}
            >
              <Trash2 />
              Delete tag
            </MenuItem>
          </Menu>
        }
      />
      {titleCut && (
        <p className="mx-auto max-w-[96rem] px-4 pt-3 text-[1.25rem] leading-snug font-semibold tracking-tight text-pretty [overflow-wrap:anywhere] md:px-8">{tag.name}</p>
      )}
      <TagPhotos key={tag.name} tag={tag} fetchPage={fetchPage} open={open} setOpen={setOpen} onChanged={load} />
      {dialog === "rename" && (
        <NameDialog
          title="Rename tag"
          label="Tag name"
          submitLabel="Rename"
          initial={tag.name}
          busy={busy}
          error={dialogError}
          onSubmit={(n) => void rename(n)}
          onCancel={() => setDialog(null)}
        />
      )}
      {dialog === "merge" && (
        <MergeDialog from={tag} tags={tags} busy={busy} error={dialogError} onMerge={(t) => void merge(t)} onCancel={() => setDialog(null)} />
      )}
      {dialog === "delete" && (
        <ConfirmSheet title={`Delete the tag “${tag.name}”?`} confirmLabel="Delete tag" danger busy={busy} onCancel={() => setDialog(null)} onConfirm={() => void remove()}>
          <p>It is taken off {tagPhotos(tag.count)}. The photos stay in your library.</p>
          {dialogError && <FormError>{dialogError}</FormError>}
        </ConfirmSheet>
      )}
      {toast}
    </>
  );
}

function TagPhotos({
  tag,
  fetchPage,
  open,
  setOpen,
  onChanged,
}: {
  tag: Tag;
  fetchPage: (cursor: string | null) => Promise<{ items: import("../api").AssetListItem[]; next_cursor: string | null; next_before: null }>;
  open: number | null;
  setOpen: (i: number | null) => void;
  onChanged: () => Promise<void>;
}) {
  const navigate = useNavigate();
  const slideshow = useSlideshow();
  const { setAnonymous } = useAuth();
  const { toast, show } = useToast();
  const { items, done, started, error, loadMore, sentinel, refreshItem, reset } = useAssetPages(fetchPage);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, undefined);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );
  const setFavorite = useCallback(
    async (id: string, favorite: boolean) => {
      refreshItem(id, { favorite });
      try {
        await api.favoriteAssets([id], favorite);
      } catch (e) {
        refreshItem(id, { favorite: !favorite });
        failure(e, "Couldn’t update the favorite.");
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
  const untagSelected = useCallback(async () => {
    const ids = sel.ids;
    if (ids.length === 0) return;
    try {
      await api.applyTags(ids, { remove: [tag.name] });
      sel.setSelecting(false);
      show({ message: `Removed ${tag.name} from ${tagPhotos(ids.length)}` });
      reset();
      void onChanged();
    } catch (e) {
      failure(e, "Couldn’t remove the tag.");
    }
  }, [sel, tag.name, show, reset, onChanged, failure]);

  if (!started) return <Loading>Loading the photos…</Loading>;
  return (
    <>
      {items.length === 0 ? (
        <Empty icon={Images} title="No photos have this tag">
          Select photos in your library and choose Tag to add it.
        </Empty>
      ) : (
        <div className="mx-auto flex max-w-[96rem] flex-wrap items-center gap-2 px-4 py-3 md:px-8">
          {slideshow && (
            <Button variant="secondary" size="sm" onClick={() => slideshow.start(scopeSource({ search: { tags: tag.name } }, tag.name))}>
              <Play />
              Slideshow
            </Button>
          )}
          <Button variant="secondary" size="sm" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
            <CheckSquare />
            Select
          </Button>
        </div>
      )}
      {sel.selecting && (
        <div className="mx-auto max-w-[96rem] px-4 md:px-8">
          <SelectionBar count={sel.count} total={items.length} onSelectAll={sel.selectAll} onClear={sel.clear} onDone={() => sel.setSelecting(false)}>
            <ZipButton
              className={buttonVariants({ variant: "quiet", size: "sm" })}
              target={sel.ids}
              disabled={sel.count === 0}
              onStarted={(message) => show({ message })}
              onError={(e) => failure(e, "Couldn’t download.")}
            />
            <RateTools
              target={sel.ids}
              count={sel.count}
              disabled={sel.count === 0}
              onDone={(rating, _n, message) => {
                for (const id of sel.ids) refreshItem(id, { rating });
                sel.setSelecting(false);
                show({ message });
              }}
              onError={(e) => failure(e, "Couldn’t rate.")}
            />
            <TagTools
              target={sel.ids}
              count={sel.count}
              disabled={sel.count === 0}
              onDone={(message) => {
                sel.setSelecting(false);
                show({ message });
                reset();
                void onChanged();
              }}
            />
            <Button variant="primary" size="sm" disabled={sel.count === 0} onClick={() => void untagSelected()}>
              Remove this tag
            </Button>
          </SelectionBar>
        </div>
      )}
      {items.length > 0 && (
        <div className="md:px-8">
          <Grid
            items={items}
            onOpen={setOpen}
            onThumbError={(i) => void api.getAsset(i).then((d) => refreshItem(i, { thumb_url: d.thumb_url, thumbhash: d.thumbhash }), () => undefined)}
            selecting={sel.selecting}
            selected={sel.selected}
            onSelect={sel.select}
            onLongPress={sel.begin}
            onFavorite={(id, favorite) => void setFavorite(id, favorite)}
          />
        </div>
      )}
      <div ref={sentinel} className="h-px" aria-hidden="true" />
      {error && <InlineError onRetry={() => void loadMore()}>{error}</InlineError>}
      {!done && items.length > 0 && (
        <p className="py-4 text-center text-sm text-muted" role="status">
          Loading more…
        </p>
      )}
      {open !== null && items.length > 0 && (
        <Lightbox
          items={items}
          index={Math.min(open, items.length - 1)}
          onIndexChange={(i) => {
            setOpen(i);
            if (i >= items.length - 3 && !done && !error) void loadMore();
          }}
          onClose={() => setOpen(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onFavorite={(it, favorite) => void setFavorite(it.id, favorite)}
          onRate={(it, n) => void rateOne(it, n)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
      {toast}
    </>
  );
}

/** Choose the tag whose name this one becomes part of. */
function MergeDialog({
  from,
  tags,
  busy,
  error,
  onMerge,
  onCancel,
}: {
  from: Tag;
  tags: Tag[];
  busy: boolean;
  error: string | null;
  onMerge: (into: Tag) => void;
  onCancel: () => void;
}) {
  const id = useId();
  const others = tags.filter((t) => t.id !== from.id);
  const [into, setInto] = useState(others[0]?.id ?? "");
  const [typed, setTyped] = useState("");
  const target = others.find((t) => t.id === into);
  const q = typed.trim().toLowerCase();
  const matches = others.filter((t) => !q || t.name.toLowerCase().includes(q));
  // A library can hold thousands of tags: the list shows the first hundred, and typing narrows it. The chosen one stays on it.
  const shown = [...(target && !matches.includes(target) ? [target] : []), ...matches].slice(0, 100);
  return (
    <Modal
      onClose={onCancel}
      title={`Merge “${from.name}” into…`}
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id} disabled={busy || !target}>
            Merge
          </Button>
        </>
      }
    >
      <form
        id={id}
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (target) onMerge(target);
        }}
      >
        {others.length > 6 && (
          <>
            <label htmlFor={`${id}-find`} className="sr-only">
              Find a tag
            </label>
            <input
              id={`${id}-find`}
              className={inputClass}
              type="search"
              placeholder="Find a tag"
              value={typed}
              maxLength={100}
              onChange={(e) => setTyped(e.target.value)}
            />
          </>
        )}
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy}>
          <legend className="sr-only">Merge into</legend>
          <div className="flex max-h-72 flex-col overflow-y-auto">
            {shown.map((t) => (
              <label key={t.id} className="flex min-h-12 cursor-pointer items-center gap-3 rounded-card px-1.5 hover:bg-surface has-[:checked]:bg-surface has-[:disabled]:cursor-default">
                <input
                  type="radio"
                  name={`${id}-into`}
                  className="size-5 shrink-0 accent-accent"
                  checked={t.id === into}
                  data-autofocus={t.id === into ? "" : undefined}
                  onChange={() => setInto(t.id)}
                />
                <span className="min-w-0 flex-1 truncate text-[0.9375rem] [overflow-wrap:anywhere]">{t.name}</span>
                <span className="shrink-0 text-[0.8125rem] text-muted">{tagPhotos(t.count)}</span>
              </label>
            ))}
          </div>
          {matches.length === 0 && <p className="py-2 text-sm text-muted">No tag has “{typed.trim()}” in its name.</p>}
        </fieldset>
        <Note>Every photo with “{from.name}” gets the other tag, and “{from.name}” is deleted.</Note>
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
  );
}
