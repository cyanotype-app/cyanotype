import { BookImage, Link2, Plus, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, ApiError, type Album, type SmartRules } from "../api";
import { useAuth } from "../auth";
import { AlbumCover } from "../components/AlbumCover";
import { NameDialog } from "../components/NameDialog";
import { Empty, InlineError, Loading, Rise } from "../components/pageKit";
import { SmartAlbumEditor } from "../components/SmartAlbumEditor";
import { shareApi } from "../shareApi";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { Menu, MenuItem } from "../ui/Menu";
import { blankRules, describeRules, smartCount } from "../smartText";
import { dateRange, hiddenNote, photoCount } from "../albumText";

/** Grid of album cards, most recently updated first. */
export function Albums() {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const [albums, setAlbums] = useState<Album[] | null>(null);
  const [shared, setShared] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [naming, setNaming] = useState(false);
  const [makingSmart, setMakingSmart] = useState(false);
  const [busy, setBusy] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .listAlbums()
      .then((r) => live && setAlbums(r.items))
      .catch((e: unknown) => {
        if (!live) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else setError(e instanceof Error ? e.message : "Could not load your albums.");
      });
    // Which albums have a link out: a small badge on the card. Nothing depends on it, so a failure is quiet.
    shareApi.list().then(
      (r) => live && setShared(new Set(r.items.filter((s) => s.kind === "album" && !s.expired && s.album).map((s) => s.album!.id))),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [setAnonymous]);

  async function create(title: string) {
    setBusy(true);
    setCreateError(null);
    try {
      const a = await api.createAlbum({ title });
      navigate(`/albums/${a.id}`);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setCreateError(e instanceof Error ? e.message : "Could not create the album.");
      setBusy(false);
    }
  }

  async function createSmart(title: string, rules: SmartRules) {
    setBusy(true);
    setCreateError(null);
    try {
      const a = await api.createSmartAlbum({ title, rules });
      navigate(`/albums/${a.id}`);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setCreateError(e instanceof Error ? e.message : "Could not create the smart album.");
      setBusy(false);
    }
  }

  return (
    <>
      <PageHeader
        title="Albums"
        actions={
          <Menu
            label="New"
            trigger={
              <Button size="icon" variant="ghost" aria-label="New">
                <Plus />
              </Button>
            }
          >
            <MenuItem
              onSelect={() => {
                setCreateError(null);
                setNaming(true);
              }}
            >
              <BookImage />
              New album
            </MenuItem>
            <MenuItem
              onSelect={() => {
                setCreateError(null);
                setMakingSmart(true);
              }}
            >
              <Sparkles />
              New smart album
            </MenuItem>
          </Menu>
        }
      />
      <div className="mx-auto max-w-[96rem] px-4 pt-2 pb-10 md:px-8">
        {error && <InlineError>{error}</InlineError>}
        {!error && albums === null && <Loading>Loading your albums…</Loading>}
        {albums !== null && albums.length === 0 && (
          <Empty
            icon={BookImage}
            title="No albums yet"
            action={
              <Button
                variant="primary"
                onClick={() => {
                  setCreateError(null);
                  setNaming(true);
                }}
              >
                <Plus />
                New album
              </Button>
            }
          >
            Group photos into albums. The photos stay in your library either way.
          </Empty>
        )}
        {albums !== null && albums.length > 0 && (
          <ul className="grid grid-cols-2 gap-x-3 gap-y-6 md:grid-cols-[repeat(auto-fill,minmax(10rem,1fr))] md:gap-x-5 md:gap-y-8" aria-label="Albums">
            {albums.map((a, i) => (
              <Rise key={a.id} index={i}>
                <Link
                  to={`/albums/${a.id}`}
                  className="group block rounded-card text-fg no-underline outline-offset-4"
                >
                  <span className="relative block">
                    <AlbumCover
                      cover={a.cover}
                      className="shadow-[0_0_0_1px_rgb(0_0_0/0.04)] transition-transform duration-200 ease-soft group-hover:scale-[1.02] group-active:scale-[0.98]"
                    />
                    {(a.smart || shared.has(a.id)) && (
                      <span className="absolute bottom-2 left-2 flex gap-1.5">
                        {a.smart && <Badge icon={Sparkles}>Smart</Badge>}
                        {shared.has(a.id) && <Badge icon={Link2}>Shared</Badge>}
                      </span>
                    )}
                  </span>
                  <span className="mt-2.5 block truncate text-[0.9375rem] leading-snug font-medium">{a.title}</span>
                  <span className="block truncate text-[0.8125rem] text-muted">
                    {[
                      a.smart ? smartCount(a.photo_count, a.photo_count_capped) : photoCount(a.photo_count),
                      hiddenNote(a.hidden_count),
                      dateRange(a.first_at, a.last_at),
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                  {a.smart && a.rules && <span className="line-clamp-1 text-[0.8125rem] text-muted">{describeRules(a.rules)}</span>}
                </Link>
              </Rise>
            ))}
          </ul>
        )}
      </div>
      {makingSmart && (
        <SmartAlbumEditor
          title="New smart album"
          submitLabel="Create smart album"
          initialRules={blankRules()}
          busy={busy}
          error={createError}
          onSubmit={(t, r) => void createSmart(t, r)}
          onCancel={() => {
            setMakingSmart(false);
            setCreateError(null);
          }}
        />
      )}
      {naming && (
        <NameDialog
          title="New album"
          label="Album title"
          submitLabel="Create"
          busy={busy}
          error={createError}
          onSubmit={(t) => void create(t)}
          onCancel={() => setNaming(false)}
        />
      )}
    </>
  );
}

/** A small frosted label on a cover: only a glance, so an icon and one word. */
function Badge({ icon: Icon, children }: { icon: typeof Sparkles; children: string }) {
  return (
    <span className="inline-flex h-6 items-center gap-1 rounded-full bg-black/45 px-2 text-xs font-medium text-white backdrop-blur-md">
      <Icon className="size-3" aria-hidden="true" />
      {children}
    </span>
  );
}
