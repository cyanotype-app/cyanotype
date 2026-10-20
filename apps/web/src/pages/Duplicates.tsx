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
    <div className="duplicates">
      <div className="page-head">
        <h1>Duplicates</h1>
        <div className="page-head-actions">
          {admin && (
            <button type="button" className="btn btn-quiet" onClick={() => void findAll()}>
              Check every account
            </button>
          )}
          <button type="button" className="btn btn-primary" onClick={() => void find()} disabled={checking}>
            {checking ? "Checking…" : "Find duplicates"}
          </button>
        </div>
      </div>
      <p className="muted small dup-status" role="status">
        {scanLine(scan)}
      </p>
      {error && (
        <p className="center" role="alert">
          {error}
        </p>
      )}
      {!error && groups === null && (
        <p className="center muted" role="status">
          Loading…
        </p>
      )}
      {groups !== null && groups.length === 0 && !checking && (
        <div className="empty">
          <h2>{scan?.state === "never" ? "Nothing checked yet" : "No duplicates to review"}</h2>
          <p>
            Photos that are the same picture twice, such as a resized or re-saved copy, show up here after a check. Bursts are not
            duplicates: they stay together in the library. <Link to="/library">Back to the library</Link>
          </p>
        </div>
      )}
      {groups !== null && groups.length > 0 && (
        <p className="muted small">
          {total.toLocaleString("en-US")} {total === 1 ? "group" : "groups"} to review. The suggested photo has the most pixels, then the most
          metadata.
        </p>
      )}
      <ul className="dup-groups" aria-label="Duplicate groups">
        {(groups ?? []).map((g, gi) => {
          const selected = chosen[g.id] ?? g.members.find((m) => m.best)?.id ?? g.members[0]!.id;
          return (
            <li key={g.id} className="dup-group card">
              <h2 className="dup-title">
                Group {gi + 1} · {g.members.length} photos
              </h2>
              <ul className="dup-members">
                {g.members.map((m, mi) => {
                  const bg = placeholderUrl(m.thumbhash);
                  const date = new Date(m.sort_time);
                  return (
                    <li key={m.id} className={`dup-member${selected === m.id ? " selected" : ""}`}>
                      <button
                        type="button"
                        className="dup-thumb"
                        style={bg ? { backgroundImage: `url(${bg})` } : undefined}
                        aria-label={`Look at ${m.original_name}`}
                        onClick={() => setLooking({ group: g, index: mi })}
                      >
                        {m.thumb_url && <img src={m.thumb_url} alt="" loading="lazy" />}
                      </button>
                      <label className="dup-pick">
                        <input type="radio" name={`keep-${g.id}`} checked={selected === m.id} onChange={() => setChosen((c) => ({ ...c, [g.id]: m.id }))} />
                        <span>
                          Keep this one
                          {m.best && <span className="badge dup-best">Suggested</span>}
                        </span>
                      </label>
                      <p className="dup-name">{m.original_name}</p>
                      <dl className="dup-facts">
                        <div>
                          <dt>Resolution</dt>
                          <dd>{resolutionLine(m.width, m.height)}</dd>
                        </div>
                        <div>
                          <dt>Size</dt>
                          <dd>{sizeLine(m, g.members)}</dd>
                        </div>
                        <div>
                          <dt>Date</dt>
                          <dd>{Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString()}</dd>
                        </div>
                        <div>
                          <dt>Metadata</dt>
                          <dd>{metadataLine(m)}</dd>
                        </div>
                      </dl>
                      <p className="dup-marks">
                        {m.favorite && (
                          <span className="dup-heart" role="img" aria-label="Favorite">
                            <HeartIcon filled />
                          </span>
                        )}
                        {(m.rating ?? 0) > 0 && <RatingStars value={m.rating} />}
                      </p>
                    </li>
                  );
                })}
              </ul>
              <div className="dup-actions">
                <button type="button" className="btn btn-quiet" disabled={busy === g.id} onClick={() => void dismiss(g)}>
                  Not duplicates
                </button>
                <button type="button" className="btn btn-primary" disabled={busy === g.id} onClick={() => void keep(g)}>
                  {keepLabel(g.members.length - 1)}
                </button>
              </div>
            </li>
          );
        })}
      </ul>
      {cursor && (
        <p className="center">
          <button type="button" className="btn btn-quiet" onClick={() => void more()} disabled={loadingMore}>
            {loadingMore ? "Loading…" : "Show more groups"}
          </button>
        </p>
      )}
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
    </div>
  );
}
