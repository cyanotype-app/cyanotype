import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type DuplicateGroup, type DuplicateScan } from "../api";
import { useAuth } from "../auth";
import { Lightbox } from "../components/Lightbox";
import { useToast } from "../components/Toast";
import { keepLabel, keptMessage, metadataLine, resolutionLine, scanLine, sizeLine } from "../dupText";
import { RatingStars } from "../components/Stars";
import { HeartIcon } from "../components/Tile";
import { placeholderUrl } from "../thumbhash";
import { Ellipsis, ScanSearch, Users } from "lucide-react";
import { Empty, Loading } from "../settings/kit";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { Menu, MenuItem } from "../ui/Menu";

/** How often the page looks again while a check is waiting or running. */
const POLL_MS = 3000;

/**
 * Near-duplicate review: the groups the worker found, each photo side by side with its resolution, size, date and how much
 * metadata it carries; the best one is preselected. "Keep selected" trashes the others after merging what they carry onto it
 * (undoable from the toast: the photos come back from the trash, the merge stays); "Not duplicates" dismisses the group for good.
 */
export function Duplicates() {
  const { state, setAnonymous } = useAuth();
  const admin = state.status === "authenticated" && state.user.role === "admin";
  const { toast, show } = useToast();
  const [groups, setGroups] = useState<DuplicateGroup[] | null>(null);
  const [total, setTotal] = useState(0);
  const [cursor, setCursor] = useState<string | null>(null);
  const [scan, setScan] = useState<DuplicateScan | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [chosen, setChosen] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [looking, setLooking] = useState<{ group: DuplicateGroup; index: number } | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  const fail = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  const load = useCallback(async () => {
    try {
      const p = await api.duplicates({ limit: 20 });
      if (!live.current) return;
      setGroups(p.groups);
      setTotal(p.total);
      setCursor(p.next_cursor);
      setScan(p.scan);
      setError(null);
    } catch (e) {
      if (!live.current) return;
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not load the duplicates.");
    }
  }, [setAnonymous]);
  useEffect(() => {
    void load();
  }, [load]);
  // While a check is waiting or running, look again; when it finishes the groups appear by themselves.
  const checking = scan?.state === "wanted" || scan?.state === "running";
  useEffect(() => {
    if (!checking) return;
    const t = setInterval(() => void load(), POLL_MS);
    return () => clearInterval(t);
  }, [checking, load]);

  const more = async () => {
    if (!cursor || loadingMore) return;
    setLoadingMore(true);
    try {
      const p = await api.duplicates({ cursor, limit: 20 });
      setGroups((g) => [...(g ?? []), ...p.groups.filter((n) => !(g ?? []).some((o) => o.id === n.id))]);
      setCursor(p.next_cursor);
    } catch (e) {
      fail(e, "Couldn’t load more.");
    } finally {
      setLoadingMore(false);
    }
  };

  const find = async () => {
    try {
      const r = await api.scanDuplicates();
      setScan(r.scan);
    } catch (e) {
      fail(e, "Couldn’t start the check.");
    }
  };
  const findAll = async () => {
    try {
      const r = await api.scanAllDuplicates();
      show({ message: `Checking ${r.accounts} ${r.accounts === 1 ? "account" : "accounts"}.` });
    } catch (e) {
      fail(e, "Couldn’t start the check.");
    }
  };

  const keep = async (g: DuplicateGroup) => {
    const keepId = chosen[g.id] ?? g.members.find((m) => m.best)?.id ?? g.members[0]!.id;
    setBusy(g.id);
    try {
      const r = await api.keepDuplicate(g.id, keepId);
      setGroups((cur) => (cur ?? []).filter((x) => x.id !== g.id));
      setTotal((n) => Math.max(0, n - 1));
      show({
        message: keptMessage(r),
        action: {
          label: "Undo",
          onClick: () =>
            void api.restoreAssets(r.trashed).then(
              () => show({ message: `Restored ${r.trashed.length}. What was merged onto the kept photo stays; check again to review them.` }),
              (e: unknown) => fail(e, "Couldn’t undo."),
            ),
        },
      });
    } catch (e) {
      fail(e, "Couldn’t keep that photo.");
      void load();
    } finally {
      setBusy(null);
    }
  };
  const dismiss = async (g: DuplicateGroup) => {
    setBusy(g.id);
    try {
      await api.dismissDuplicates(g.id);
      setGroups((cur) => (cur ?? []).filter((x) => x.id !== g.id));
      setTotal((n) => Math.max(0, n - 1));
      show({ message: "Marked as not duplicates. They will not be offered again." });
    } catch (e) {
      fail(e, "Couldn’t dismiss the group.");
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <PageHeader
        title="Duplicates"
        actions={
          <>
            {admin && (
              <Menu
                label="More"
                trigger={
                  <Button variant="ghost" size="icon" aria-label="More">
                    <Ellipsis />
                  </Button>
                }
              >
                <MenuItem onSelect={() => void findAll()}>
                  <Users />
                  Check every account
                </MenuItem>
              </Menu>
            )}
            <Button variant="ghost" size="icon" aria-label={checking ? "Checking…" : "Find duplicates"} disabled={checking} onClick={() => void find()}>
              <ScanSearch className={checking ? "animate-pulse" : undefined} />
            </Button>
          </>
        }
      />
      <div className="px-4 pb-10 md:px-8">
        <p className="min-h-5 text-sm text-muted" role="status">
          {scanLine(scan)}
        </p>
        {error && (
          <p className="py-6 text-center text-sm" role="alert">
            {error}
          </p>
        )}
        {!error && groups === null && <Loading />}
        {groups !== null && groups.length === 0 && !checking && (
          <Empty title={scan?.state === "never" ? "Nothing checked yet" : "No duplicates to review"}>
            <p>
              Photos that are the same picture twice, such as a resized or re-saved copy, show up here after a check. Bursts are not
              duplicates: they stay together in the library.
            </p>
            <p className="mt-3">
              <Link to="/photos" className="text-accent">
                Back to the library
              </Link>
            </p>
          </Empty>
        )}
        {groups !== null && groups.length > 0 && (
          <p className="mt-1 text-sm text-muted">
            {total.toLocaleString("en-US")} {total === 1 ? "group" : "groups"} to review. The suggested photo has the most pixels, then the
            most metadata.
          </p>
        )}
        <ul className="mt-4 flex flex-col gap-5" aria-label="Duplicate groups">
          {(groups ?? []).map((g, gi) => {
            const selected = chosen[g.id] ?? g.members.find((m) => m.best)?.id ?? g.members[0]!.id;
            return (
              <li key={g.id} className="rounded-card bg-card p-4 ring-1 ring-line md:p-5">
                <h2 className="text-[1.0625rem] font-semibold tracking-tight">
                  Group {gi + 1} · {g.members.length} photos
                </h2>
                <ul className={cn("mt-3 grid gap-3", g.members.length === 2 ? "grid-cols-2" : "grid-cols-2 md:grid-cols-3 lg:grid-cols-4")}>
                  {g.members.map((m, mi) => {
                    const bg = placeholderUrl(m.thumbhash);
                    const date = new Date(m.sort_time);
                    const on = selected === m.id;
                    return (
                      <li key={m.id} className="flex min-w-0 flex-col gap-2">
                        <button
                          type="button"
                          className={cn(
                            "relative aspect-square w-full overflow-hidden rounded-tile bg-surface bg-cover transition-shadow duration-150",
                            on ? "ring-2 ring-accent ring-offset-2 ring-offset-card" : "ring-0",
                          )}
                          style={bg ? { backgroundImage: `url(${bg})` } : undefined}
                          aria-label={`Look at ${m.original_name}`}
                          onClick={() => setLooking({ group: g, index: mi })}
                        >
                          {m.thumb_url && <img src={m.thumb_url} alt="" loading="lazy" className="size-full object-cover" />}
                          {(m.favorite || (m.rating ?? 0) > 0) && (
                            <span className="absolute bottom-1.5 left-1.5 flex items-center gap-1.5 rounded-full bg-black/45 px-2 py-1 text-white backdrop-blur-sm">
                              {m.favorite && (
                                <span className="inline-block size-4" role="img" aria-label="Favorite">
                                  <HeartIcon filled />
                                </span>
                              )}
                              {(m.rating ?? 0) > 0 && <RatingStars value={m.rating} />}
                            </span>
                          )}
                        </button>
                        <label className="flex cursor-pointer items-start gap-2 text-[0.9375rem]">
                          <input
                            type="radio"
                            className="mt-1 size-4 shrink-0 accent-accent"
                            name={`keep-${g.id}`}
                            checked={on}
                            onChange={() => setChosen((c) => ({ ...c, [g.id]: m.id }))}
                          />
                          <span className="min-w-0">
                            Keep this one
                            {m.best && (
                              <span className="ml-1.5 inline-flex h-5 items-center rounded-full bg-accent/12 px-2 align-middle text-xs font-medium text-accent">
                                Suggested
                              </span>
                            )}
                          </span>
                        </label>
                        <p className="truncate text-[0.8125rem] text-muted" title={m.original_name}>
                          {m.original_name}
                        </p>
                        <dl className="grid grid-cols-1 gap-y-1.5 text-[0.8125rem]">
                          {(
                            [
                              ["Resolution", resolutionLine(m.width, m.height)],
                              ["Size", sizeLine(m, g.members)],
                              ["Date", Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString()],
                              ["Metadata", metadataLine(m)],
                            ] as const
                          ).map(([k, v]) => (
                            <div key={k} className="min-w-0">
                              <dt className="text-xs text-muted">{k}</dt>
                              <dd className="break-words">{v}</dd>
                            </div>
                          ))}
                        </dl>
                      </li>
                    );
                  })}
                </ul>
                <div className="mt-4 flex flex-col-reverse gap-2 min-[480px]:flex-row min-[480px]:justify-end">
                  <Button disabled={busy === g.id} onClick={() => void dismiss(g)}>
                    Not duplicates
                  </Button>
                  <Button variant="primary" disabled={busy === g.id} onClick={() => void keep(g)}>
                    {keepLabel(g.members.length - 1)}
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
        {cursor && (
          <div className="mt-5 flex justify-center">
            <Button onClick={() => void more()} disabled={loadingMore}>
              {loadingMore ? "Loading…" : "Show more groups"}
            </Button>
          </div>
        )}
      </div>
      {looking && (
        <Lightbox
          items={looking.group.members}
          index={looking.index}
          onIndexChange={(i) => setLooking({ group: looking.group, index: i })}
          onClose={() => setLooking(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
        />
      )}
      {toast}
    </>
  );
}
