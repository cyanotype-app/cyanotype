import { useEffect, useId, useState, type FormEvent } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api, ApiError } from "../api";
import { Logo } from "../components/Logo";

const PROBLEM: Record<string, string> = {
  unknown: "This reset link is not valid. Check that you copied all of it, or ask the administrator for a new one.",
  used: "This reset link was already used. Ask the administrator for a new one.",
  expired: "This reset link has expired. Ask the administrator for a new one.",
  disabled: "This account is disabled. Ask the administrator.",
};

type State = { status: "checking" } | { status: "ready"; email: string } | { status: "problem"; reason: string };

/** The page a password reset link opens: choose a new password, once. No sign-in needed. */
export function ResetPassword() {
  const id = useId();
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
    <div className="auth-page">
      <div className="card auth-card">
        <div className="auth-brand">
          <Logo size={44} />
          <h1>Choose a new password</h1>
        </div>
        {done ? (
          <>
            <p className="notice notice-ok" role="status">
              Your password is changed and you were signed out everywhere.
            </p>
            <p className="auth-footer">
              <Link to="/login">Sign in with the new password</Link>
            </p>
          </>
        ) : state.status === "problem" ? (
          <p className="notice" role="alert">
            {PROBLEM[state.reason] ?? PROBLEM.unknown}
          </p>
        ) : (
          <form onSubmit={submit}>
            {state.status === "ready" && state.email && <p className="muted small">For {state.email}</p>}
            <label htmlFor={`${id}-p`}>New password</label>
            <input id={`${id}-p`} type="password" autoComplete="new-password" required minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} />
            <p className="muted small">At least 8 characters.</p>
            <label htmlFor={`${id}-a`}>Again</label>
            <input id={`${id}-a`} type="password" autoComplete="new-password" required value={again} onChange={(e) => setAgain(e.target.value)} />
            <div role="alert" className="form-error">
              {error}
            </div>
            <button type="submit" className="btn btn-primary" disabled={busy || state.status === "checking"}>
              {busy ? "Please wait…" : "Set the password"}
            </button>
          </form>
        )}
      </div>
    </div>
  );
}
