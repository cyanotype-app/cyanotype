import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type HiddenState } from "../api";
import { useAuth } from "../auth";
import { EyeOff } from "lucide-react";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
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

  if (state === null) {
    return (
      <div>
        <PageHeader title="Hidden" />
        <p className="py-24 text-center text-muted" role="status">Loading…</p>
      </div>
    );
  }
  if (!state.unlocked) return <UnlockPrompt onUnlocked={setState} />;
  return (
    <Library
      hidden
      onLocked={lock}
      subbar={
        <div className="flex items-center gap-2 px-4 py-1.5 md:px-8" role="status">
          <span className="min-w-0 flex-1 text-sm text-muted">{state.expires_at ? unlockedUntil(state.expires_at) : "Unlocked"}</span>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              void api.lockHidden().catch(() => undefined);
              lock();
            }}
          >
            Lock now
          </Button>
        </div>
      }
    />
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
    <div>
      <PageHeader title="Hidden" />
      <form
        className="mx-auto flex max-w-sm flex-col items-center px-6 pt-[10vh] pb-10 text-center"
        onSubmit={(e) => void submit(e)}
        aria-labelledby="unlock-title"
      >
        <span className="mb-5 grid size-16 grid-cols-1 place-items-center rounded-full bg-surface text-accent">
          <EyeOff className="size-7" strokeWidth={1.6} aria-hidden="true" />
        </span>
        <h2 id="unlock-title" className="text-[1.1875rem] font-semibold tracking-tight">
          Hidden photos
        </h2>
        <p className="mt-1.5 text-[0.9375rem] text-muted">Enter your password to open the photos you hid. They stay open for 10 minutes.</p>
        <label htmlFor="unlock-password" className="mt-6 self-start text-sm font-medium">
          Password
        </label>
        <input
          id="unlock-password"
          ref={input}
          className="mt-1.5 h-11 w-full rounded-xl border border-line bg-card px-3.5 text-[0.9375rem] text-fg"
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          aria-invalid={error !== null}
          aria-describedby={error ? "unlock-error" : undefined}
        />
        {error && (
          <p id="unlock-error" className="mt-2 self-start text-sm text-danger" role="alert">
            {error}
          </p>
        )}
        <Button type="submit" variant="primary" className="mt-5 w-full" disabled={!password || busy}>
          {busy ? "Opening…" : "Open Hidden"}
        </Button>
      </form>
    </div>
  );
}
