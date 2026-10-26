import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type HiddenState } from "../api";
import { useAuth } from "../auth";
import { Lock, LockOpen } from "lucide-react";
import { Field, inputClass, Loading } from "../settings/kit";
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

  if (state === null) return <Loading>Loading…</Loading>;
  if (!state.unlocked) return <UnlockPrompt onUnlocked={setState} />;
  return (
    <div>
      <div role="status" className="flex items-center gap-2 border-b border-line bg-surface px-4 py-1.5 md:px-8">
        <LockOpen className="size-4 shrink-0 text-muted" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate text-sm text-muted">{state.expires_at ? unlockedUntil(state.expires_at) : "Unlocked"}</span>
        <Button
          size="sm"
          variant="quiet"
          onClick={() => {
            void api.lockHidden().catch(() => undefined);
            lock();
          }}
        >
          Lock now
        </Button>
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
    <>
      <PageHeader title="Hidden" />
      <div className="mx-auto flex max-w-sm flex-col items-center px-5 pt-10 pb-16 md:pt-20">
        <span className="mb-5 inline-flex size-14 items-center justify-center rounded-full bg-surface text-muted">
          <Lock className="size-6" aria-hidden="true" />
        </span>
        <form className="flex w-full flex-col gap-4" onSubmit={(e) => void submit(e)} aria-label="Open Hidden">
          <p className="text-center text-[0.9375rem] text-muted">Enter your password to open the photos you hid. They stay open for 10 minutes.</p>
          <Field label="Password" error={error}>
            {(p) => (
              <input
                {...p}
                ref={input}
                className={inputClass}
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            )}
          </Field>
          <Button type="submit" variant="primary" size="lg" disabled={!password || busy}>
            {busy ? "Opening…" : "Open Hidden"}
          </Button>
        </form>
      </div>
    </>
  );
}
