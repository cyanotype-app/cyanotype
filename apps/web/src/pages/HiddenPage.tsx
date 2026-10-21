import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type HiddenState } from "../api";
import { useAuth } from "../auth";
import { LibraryTabs } from "../components/LibraryTabs";
import { msUntil, unlockedUntil } from "../viewText";
import { Library } from "./Library";

/**
 * The Hidden view. It asks for the account password first; the server keeps the unlock on the login session for ten
 * minutes and refuses every hidden-list request without it, so this page only decides what to draw. When the time runs
 * out (or a request is refused as locked) it asks again.
 */
export function HiddenPage() {
  const { setAnonymous } = useAuth();
  const [state, setState] = useState<HiddenState | null>(null);

  useEffect(() => {
    let live = true;
    api
      .hiddenState()
      .then((s) => live && setState(s))
      .catch((e) => {
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else if (live) setState({ unlocked: false, expires_at: null });
      });
    return () => {
      live = false;
    };
  }, [setAnonymous]);

  const lock = useCallback(() => setState({ unlocked: false, expires_at: null }), []);
  // Locked by the clock: the prompt comes back when the ten minutes are up.
  useEffect(() => {
    if (!state?.unlocked || !state.expires_at) return;
    const t = setTimeout(lock, msUntil(state.expires_at));
    return () => clearTimeout(t);
  }, [state, lock]);

  if (state === null) return <p className="center muted" role="status">Loading…</p>;
  if (!state.unlocked) return <UnlockPrompt onUnlocked={setState} />;
  return (
    <div className="hidden-open">
      <div className="hidden-bar" role="status">
        <span className="muted small">{state.expires_at ? unlockedUntil(state.expires_at) : "Unlocked"}</span>
        <button
          type="button"
          className="btn btn-quiet"
          onClick={() => {
            void api.lockHidden().catch(() => undefined);
            lock();
          }}
        >
          Lock now
        </button>
      </div>
      <Library hidden onLocked={lock} />
    </div>
  );
}

/** "Enter your password to open Hidden": the account password, checked by the server. */
export function UnlockPrompt({ onUnlocked }: { onUnlocked: (s: HiddenState) => void }) {
  const { setAnonymous } = useAuth();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!password || busy) return;
    setBusy(true);
    setError(null);
    try {
      onUnlocked(await api.unlockHidden(password));
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) setAnonymous();
      else if (err instanceof ApiError && err.code === "wrong_password") setError("That password is not right.");
      else if (err instanceof ApiError && err.status === 429) setError("Too many tries. Wait a moment and try again.");
      else setError(err instanceof Error ? err.message : "Couldn’t unlock.");
      setPassword("");
      input.current?.focus();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="library">
      <LibraryTabs />
      <form className="unlock card" onSubmit={(e) => void submit(e)} aria-labelledby="unlock-title">
        <h1 id="unlock-title">Hidden</h1>
        <p className="muted">Enter your password to open the photos you hid. They stay open for 10 minutes.</p>
        <label htmlFor="unlock-password">Password</label>
        <input
          id="unlock-password"
          ref={input}
          className="text-input"
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          aria-invalid={error !== null}
          aria-describedby={error ? "unlock-error" : undefined}
        />
        {error && (
          <p id="unlock-error" className="form-error" role="alert">
            {error}
          </p>
        )}
        <button type="submit" className="btn btn-primary" disabled={!password || busy}>
          {busy ? "Opening…" : "Open Hidden"}
        </button>
      </form>
    </div>
  );
}
