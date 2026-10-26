import { Plus, Search } from "lucide-react";
import { useEffect, useId, useMemo, useState, type FormEvent } from "react";
import { api, ApiError, isScope, type Album, type Target } from "../api";
import { useAuth } from "../auth";
import { photoCount } from "../albumText";
import { SAVE_ALBUM_NOTE } from "../peopleText";
import { Button } from "../ui/Button";
import { AlbumCover } from "./AlbumCover";
import { FormError, inputClass, Modal, Note, TextInput } from "./sheetForm";

interface Props {
  /** The photos to add: explicit ids, or a whole view. */
  ids: Target;
  /** How many photos that is (shown in the title). */
  count: number;
  onClose: () => void;
  /** `count` is how many were new to the album; `created` when the album was made by this dialog. */
  onDone: (result: { album: Album; count: number; created: boolean }) => void;
  /** `save`: only make a new album from the photos ("Save as album"), starting at its title. The default also adds to an existing one. */
  mode?: "add" | "save";
}

/** A sheet to pick an album (the most recent first, with search) or make a new one, then add `ids` to it. */
export function AddToAlbumDialog({ ids, count, onClose, onDone, mode = "add" }: Props) {
  const save = mode === "save";
  const { setAnonymous } = useAuth();
  const form = useId();
  const [albums, setAlbums] = useState<Album[] | null>(null);
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(save);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (save) {
      setAlbums([]); // (nothing to pick from: the dialog only makes a new album)
      return;
    }
    let live = true;
    api
      .listAlbums()
      // A smart album gets its photos from its rules: it is not somewhere to add a photo.
      .then((r) => live && setAlbums(r.items.filter((a) => !a.smart)))
      .catch((e: unknown) => {
        if (!live) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else setError(e instanceof Error ? e.message : "Could not load your albums.");
        setAlbums([]);
      });
    return () => {
      live = false;
    };
  }, [setAnonymous, save]);

  useEffect(() => {
    if (creating) document.querySelector<HTMLElement>("dialog[open] [data-autofocus]")?.focus({ preventScroll: true });
  }, [creating]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (albums ?? []).filter((a) => a.title.toLowerCase().includes(q));
  }, [albums, query]);

  async function run(task: () => Promise<{ album: Album; count: number; created: boolean }>) {
    setBusy(true);
    setError(null);
    try {
      onDone(await task());
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not add the photos.");
      setBusy(false);
    }
  }

  const addTo = (album: Album) =>
    run(async () => ({ album, count: (await api.addToAlbum(album.id, ids)).count, created: false }));

  const create = (e: FormEvent) => {
    e.preventDefault();
    const t = title.trim();
    if (!t) return;
    void run(async () => {
      if (isScope(ids)) {
        // A whole view: create the album empty, then let the server add the photos in one go.
        const album = await api.createAlbum({ title: t });
        const added = await api.addToAlbum(album.id, ids);
        return { album, count: added.count, created: true };
      }
      const album = await api.createAlbum({ title: t, asset_ids: ids });
      return { album, count: album.photo_count, created: true };
    });
  };

  const heading = save ? `Save ${photoCount(count)} as an album` : `Add ${photoCount(count)} to album`;

  const footer = creating ? (
    <>
      <Button variant="secondary" onClick={() => (save ? onClose() : setCreating(false))} disabled={busy}>
        {save ? "Cancel" : "Back"}
      </Button>
      <Button variant="primary" type="submit" form={form} disabled={busy || !title.trim()}>
        {save ? "Create album" : "Create and add"}
      </Button>
    </>
  ) : undefined;

  return (
    <Modal onClose={onClose} title={heading} size={creating ? "sm" : "md"} footer={footer} className={creating ? undefined : "md:h-[34rem]"}>
      {creating ? (
        <form id={form} onSubmit={create} className="flex flex-col gap-3">
          <label htmlFor={`${form}-title`} className="sr-only">
            Album title
          </label>
          <TextInput
            id={`${form}-title`}
            data-autofocus=""
            placeholder="Album title"
            value={title}
            maxLength={200}
            onChange={(e) => setTitle(e.target.value)}
            disabled={busy}
          />
          {save && <Note>{SAVE_ALBUM_NOTE}</Note>}
          {error && <FormError>{error}</FormError>}
        </form>
      ) : (
        <div className="flex min-h-0 flex-col gap-3">
          <div className="relative">
            <Search className="pointer-events-none absolute top-1/2 left-3.5 size-4 -translate-y-1/2 text-muted" aria-hidden="true" />
            <label htmlFor={`${form}-search`} className="sr-only">
              Search albums
            </label>
            <input
              id={`${form}-search`}
              className={`${inputClass} pl-10`}
              type="search"
              placeholder="Search albums"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              disabled={busy}
            />
          </div>
          {error && <FormError>{error}</FormError>}
          <button
            type="button"
            className="flex min-h-14 items-center gap-3 rounded-card px-1.5 text-left text-[0.9375rem] font-medium text-accent hover:bg-surface disabled:opacity-55"
            disabled={busy}
            onClick={() => {
              setTitle(query.trim());
              setCreating(true);
            }}
          >
            <span className="flex size-11 shrink-0 items-center justify-center rounded-[0.6rem] bg-surface">
              <Plus className="size-5" aria-hidden="true" />
            </span>
            New album
          </button>
          <ul aria-label="Albums" className="flex flex-col">
            {albums === null && (
              <li className="py-2 text-sm text-muted" role="status">
                Loading…
              </li>
            )}
            {albums !== null && shown.length === 0 && (
              <li className="py-2 text-sm text-muted">{albums.length === 0 ? "No albums yet." : "No album matches."}</li>
            )}
            {shown.map((a) => (
              <li key={a.id}>
                <button
                  type="button"
                  className="flex min-h-14 w-full items-center gap-3 rounded-card px-1.5 text-left hover:bg-surface disabled:opacity-55"
                  onClick={() => void addTo(a)}
                  disabled={busy}
                >
                  <AlbumCover cover={a.cover} className="size-11 shrink-0 rounded-[0.6rem]" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[0.9375rem]">{a.title}</span>
                    <span className="block text-[0.8125rem] text-muted">{photoCount(a.photo_count)}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Modal>
  );
}

/** "Added 3 to Trip", or what happened instead when nothing was new. */
export function addedMessage(r: { album: Album; count: number; created: boolean }, requested: number): string {
  if (r.created) return `Created ${r.album.title} with ${photoCount(r.count)}`;
  if (r.count === 0) return `${requested === 1 ? "That photo is" : "Those photos are"} already in ${r.album.title}`;
  return `Added ${r.count} to ${r.album.title}`;
}
