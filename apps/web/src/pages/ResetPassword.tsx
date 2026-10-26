import { useEffect, useState, type FormEvent } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api, ApiError } from "../api";
import { AuthShell } from "../components/AuthShell";
import { Field, FormError, inputClass, Notice } from "../settings/kit";
import { Button } from "../ui/Button";

const PROBLEM: Record<string, string> = {
  unknown: "This reset link is not valid. Check that you copied all of it, or ask the administrator for a new one.",
  used: "This reset link was already used. Ask the administrator for a new one.",
  expired: "This reset link has expired. Ask the administrator for a new one.",
  disabled: "This account is disabled. Ask the administrator.",
};

type State = { status: "checking" } | { status: "ready"; email: string } | { status: "problem"; reason: string };

/** The page a password reset link opens: choose a new password, once. No sign-in needed. */
export function ResetPassword() {
  const [params] = useSearchParams();
  const token = (params.get("token") ?? "").trim();
  const [state, setState] = useState<State>(token ? { status: "checking" } : { status: "problem", reason: "unknown" });
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);

  useEffect(() => {
    if (!token) return;
    let live = true;
    api
      .resetCheck(token)
      .then((r) => live && setState(r.valid ? { status: "ready", email: r.email ?? "" } : { status: "problem", reason: r.reason ?? "unknown" }))
      .catch(() => live && setState({ status: "ready", email: "" })); // the form will say if it cannot work
    return () => {
      live = false;
    };
  }, [token]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (password.length < 8) return setError("Use at least 8 characters.");
    if (password !== again) return setError("The two passwords are not the same.");
    setBusy(true);
    setError(null);
    try {
      await api.resetPassword(token, password);
      setDone(true);
    } catch (err) {
      setError(
        err instanceof ApiError && err.status === 429
          ? "Too many attempts. Wait a minute and try again."
          : err instanceof Error
            ? err.message
            : "Could not set the password.",
      );
      setBusy(false);
    }
  }

  return (
    <AuthShell title="Choose a new password">
      {done ? (
        <div className="flex flex-col gap-4">
          <Notice tone="ok" role="status">
            Your password is changed and you were signed out everywhere.
          </Notice>
          <Button variant="primary" size="lg" className="w-full" asChild>
            <Link to="/login">Sign in with the new password</Link>
          </Button>
        </div>
      ) : state.status === "problem" ? (
        <Notice tone="warn" role="alert">
          {PROBLEM[state.reason] ?? PROBLEM.unknown}
        </Notice>
      ) : (
        <form onSubmit={submit} className="flex flex-col gap-4">
          {state.status === "ready" && state.email && <p className="text-sm text-muted">For {state.email}</p>}
          <Field label="New password" hint="At least 8 characters.">
            {(p) => (
              <input {...p} className={inputClass} type="password" autoComplete="new-password" required minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} />
            )}
          </Field>
          <Field label="Again">
            {(p) => (
              <input {...p} className={inputClass} type="password" autoComplete="new-password" required value={again} onChange={(e) => setAgain(e.target.value)} />
            )}
          </Field>
          <FormError>{error}</FormError>
          <Button type="submit" variant="primary" size="lg" className="w-full" disabled={busy || state.status === "checking"}>
            {busy ? "Please wait…" : "Set the password"}
          </Button>
        </form>
      )}
    </AuthShell>
  );
}
