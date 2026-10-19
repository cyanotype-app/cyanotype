import { useCallback, useEffect, useState } from "react";
import { api, type SessionInfo } from "../api";
import { useAuthOptional } from "../auth";

const when = (iso: string) => new Date(iso).toLocaleString();

/** Settings > Sessions: where the account is signed in, with "sign out everywhere". */
export function Sessions() {
  const auth = useAuthOptional();
  const [items, setItems] = useState<SessionInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setItems((await api.mySessions()).items);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load your sessions.");
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  async function run(what: () => Promise<unknown>, done: string, signedOut = false) {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      await what();
      if (signedOut) {
        auth?.setAnonymous();
        return;
      }
      setNote(done);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "That did not work.");
    } finally {
      setBusy(false);
    }
  }

  const others = items?.filter((s) => !s.current).length ?? 0;
  return (
    <div className="settings">
      <h1>Sessions</h1>
      <p className="muted">Each browser you signed in on is a session; it lasts 30 days. End the ones you do not recognise.</p>
      <div role="alert" className="form-error">
        {error}
      </div>
      {note && (
        <p className="small" role="status">
          {note}
        </p>
      )}
      {items === null ? (
        !error && (
          <p className="muted" role="status">
            Loading…
          </p>
        )
      ) : (
        <>
          <ul className="key-list">
            {items.map((s) => (
              <li key={s.id}>
                <div>
                  <strong>{s.current ? "This browser" : "Another browser"}</strong>
                  <div className="muted small">
                    Signed in {when(s.created_at)} · expires {when(s.expires_at)}
                  </div>
                </div>
                {!s.current && (
                  <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => void run(() => api.revokeSession(s.id), "Signed out.")}>
                    Sign out
                  </button>
                )}
              </li>
            ))}
          </ul>
          <div className="invite-actions">
            {others > 0 && (
              <button
                type="button"
                className="btn btn-quiet"
                disabled={busy}
                onClick={() => void run(() => api.revokeOtherSessions(), "Signed out everywhere else.")}
              >
                Sign out the others
              </button>
            )}
            <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => void run(() => api.revokeAllSessions(), "", true)}>
              Sign out everywhere
            </button>
          </div>
        </>
      )}
    </div>
  );
}
