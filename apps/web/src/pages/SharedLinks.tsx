import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ApiError } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { EditShareDialog } from "../components/ShareDialog";
import { useToast } from "../components/Toast";
import { shareApi, type ShareLink } from "../shareApi";
import { copyText, downloadText, shareName, shareSummary, shareUrl } from "../shareText";

/** The owner's share links: copy, edit expiry / password / downloads, revoke. */
export function SharedLinks() {
  const { setAnonymous } = useAuth();
  const { toast, show } = useToast();
  const [links, setLinks] = useState<ShareLink[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<ShareLink | null>(null);
  const [revoking, setRevoking] = useState<ShareLink | null>(null);
  const [busy, setBusy] = useState(false);

  const fail = useCallback(
    (e: unknown, fallback: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else return e instanceof Error ? e.message : fallback;
      return null;
    },
    [setAnonymous],
  );

  useEffect(() => {
    let live = true;
    shareApi
      .list()
      .then((r) => live && setLinks(r.items))
      .catch((e: unknown) => live && setError(fail(e, "Could not load your shared links.")));
    return () => {
      live = false;
    };
  }, [fail]);

  async function copy(s: ShareLink) {
    const ok = await copyText(shareUrl(s.token));
    show(ok ? { message: "Link copied" } : { message: "Could not copy. Open the link and copy it from the address bar.", tone: "error" });
  }

  async function revoke(s: ShareLink) {
    setBusy(true);
    try {
      await shareApi.revoke(s.id);
      setLinks((l) => l?.filter((x) => x.id !== s.id) ?? l);
      setRevoking(null);
      show({ message: "Link revoked. It no longer works." });
    } catch (e) {
      setRevoking(null);
      const msg = fail(e, "Could not revoke the link.");
      if (msg) show({ message: msg, tone: "error" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="albums shared-links">
      <div className="page-head">
        <h1>Shared links</h1>
        <Link to="/albums" className="btn btn-quiet">
          Albums
        </Link>
      </div>
      <p className="muted small share-intro">
        Anyone with a link can view what it shares, without an account. Revoking a link ends access at once.
      </p>
      {error && (
        <p className="center" role="alert">
          {error}
        </p>
      )}
      {!error && links === null && <p className="center muted" role="status">Loading your links…</p>}
      {links !== null && links.length === 0 && (
        <div className="empty">
          <h2>No shared links yet</h2>
          <p>Select photos or open an album and choose Share to create one.</p>
        </div>
      )}
      {links !== null && links.length > 0 && (
        <ul className="share-list" aria-label="Shared links">
          {links.map((s) => (
            <li key={s.id} className={`share-row${s.expired ? " expired" : ""}`}>
              <div className="share-row-head">
                <strong className="share-row-title">{shareName(s)}</strong>
                {s.expired && <span className="share-badge share-badge-warn">Expired</span>}
                {s.has_password && <span className="share-badge">Password</span>}
                {s.include_originals && <span className="share-badge share-badge-warn">Originals</span>}
              </div>
              <p className="muted small share-row-meta">{shareSummary(s)}</p>
              <p className="muted small share-row-meta">{downloadText(s)}</p>
              <input
                className="text-input share-url"
                readOnly
                aria-label={`Link for ${shareName(s)}`}
                value={shareUrl(s.token)}
                onFocus={(e) => e.currentTarget.select()}
              />
              <div className="share-row-actions">
                <button type="button" className="btn btn-quiet" onClick={() => void copy(s)}>
                  Copy link
                </button>
                <a className="btn btn-quiet" href={shareUrl(s.token)} target="_blank" rel="noopener noreferrer">
                  Open
                </a>
                <button type="button" className="btn btn-quiet" onClick={() => setEditing(s)}>
                  Edit
                </button>
                <button type="button" className="btn btn-quiet btn-danger-quiet" onClick={() => setRevoking(s)}>
                  Revoke
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {editing && (
        <EditShareDialog
          share={editing}
          onClose={() => setEditing(null)}
          onSaved={(updated) => {
            setLinks((l) => l?.map((x) => (x.id === updated.id ? updated : x)) ?? l);
            setEditing(null);
            show({ message: "Link updated" });
          }}
        />
      )}
      {revoking && (
        <ConfirmDialog
          title="Revoke this link?"
          confirmLabel="Revoke link"
          danger
          busy={busy}
          onCancel={() => setRevoking(null)}
          onConfirm={() => void revoke(revoking)}
        >
          <p>Anyone who has the link will no longer be able to open it. Your photos are not touched.</p>
        </ConfirmDialog>
      )}
      {toast}
    </div>
  );
}
