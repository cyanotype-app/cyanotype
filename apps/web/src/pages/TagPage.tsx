import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, type Tag } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { NameDialog } from "../components/NameDialog";
import { RateTools } from "../components/RateTools";
import { ZipButton } from "../components/ZipButton";
import { SelectionBar } from "../components/SelectionBar";
import { TagTools } from "../components/TagTools";
import { useToast } from "../components/Toast";
import { useSelection } from "../selection";
import { similarUrl } from "../searchText";
import { tagPhotos, tagSearchHref } from "../tagText";
import { useAssetPages } from "../useAssetPages";

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
      <div className="empty" role="alert">
        <h1>Couldn’t load the tag</h1>
        <p className="muted">{problem}</p>
        <Link to="/tags" className="btn btn-primary">
          All tags
        </Link>
      </div>
    );
  }
  if (tags === null) return <p className="center muted" role="status">Loading the tag…</p>;
  if (!tag) {
    return (
      <div className="empty" role="alert">
        <h1>Tag not found</h1>
        <p className="muted">It may have been renamed into another tag or deleted.</p>
        <Link to="/tags" className="btn btn-primary">
          All tags
        </Link>
      </div>
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
    <div className="library tag-page">
      <p className="small">
        <Link to="/tags">‹ Tags</Link>
      </p>
      <div className="page-head album-head">
        <div>
          <h1 className="tag-title">{tag.name}</h1>
          <p className="muted small">{tagPhotos(tag.count)}</p>
        </div>
        <div className="album-head-actions">
          <button type="button" className="btn btn-quiet" onClick={() => setDialog("rename")}>
            Rename
          </button>
          <button type="button" className="btn btn-quiet" onClick={() => setDialog("merge")} disabled={tags.length < 2}>
            Merge into…
          </button>
          <Link to={tagSearchHref(tag.name)} className="btn btn-quiet">
            Search with filters
          </Link>
          <button type="button" className="btn btn-quiet btn-danger-quiet" onClick={() => setDialog("delete")}>
            Delete tag
          </button>
        </div>
      </div>
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
        <ConfirmDialog title={`Delete the tag “${tag.name}”?`} confirmLabel="Delete tag" danger busy={busy} onCancel={() => setDialog(null)} onConfirm={() => void remove()}>
          <p>It is taken off {tagPhotos(tag.count)}. The photos stay in your library.</p>
          {dialogError && (
            <p role="alert" className="form-error">
              {dialogError}
            </p>
          )}
        </ConfirmDialog>
      )}
      {toast}
    </div>
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

  if (!started) return <p className="center muted" role="status">Loading the photos…</p>;
  return (
    <>
      {items.length === 0 ? (
        <div className="empty">
          <h2>No photos have this tag</h2>
          <p>Select photos in your library and choose Tag to add it.</p>
        </div>
      ) : (
        <div className="library-bar">
          <button type="button" className="btn btn-quiet" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
            Select
          </button>
        </div>
      )}
      {sel.selecting && (
        <SelectionBar count={sel.count} total={items.length} onSelectAll={sel.selectAll} onClear={sel.clear} onDone={() => sel.setSelecting(false)}>
          <ZipButton
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
          <button type="button" className="btn btn-primary" disabled={sel.count === 0} onClick={() => void untagSelected()}>
            Remove this tag
          </button>
        </SelectionBar>
      )}
      {items.length > 0 && (
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
      )}
      <div ref={sentinel} className="sentinel" aria-hidden="true" />
      {error && (
        <p className="center" role="alert">
          {error}{" "}
          <button type="button" className="btn btn-quiet" onClick={() => void loadMore()}>
            Retry
          </button>
        </p>
      )}
      {!done && items.length > 0 && <p className="center muted small" role="status">Loading more…</p>}
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
  const others = tags.filter((t) => t.id !== from.id);
  const [into, setInto] = useState(others[0]?.id ?? "");
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);
  const target = others.find((t) => t.id === into);
  return (
    <div className="modal-backdrop">
      <form
        className="modal card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="merge-title"
        onSubmit={(e) => {
          e.preventDefault();
          if (target) onMerge(target);
        }}
      >
        <h2 id="merge-title">Merge “{from.name}” into…</h2>
        <label htmlFor="merge-into" className="sr-only">
          Merge into
        </label>
        <select id="merge-into" className="text-input" value={into} onChange={(e) => setInto(e.target.value)} disabled={busy} autoFocus>
          {others.map((t) => (
            <option key={t.id} value={t.id}>
              {t.name} ({tagPhotos(t.count)})
            </option>
          ))}
        </select>
        <p className="muted small">Every photo with “{from.name}” gets the other tag, and “{from.name}” is deleted.</p>
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || !target}>
            Merge
          </button>
        </div>
      </form>
    </div>
  );
}
