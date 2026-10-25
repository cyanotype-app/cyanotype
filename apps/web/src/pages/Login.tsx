import { useId, useState, type FormEvent } from "react";
import { Link, Navigate, useLocation, useNavigate } from "react-router-dom";
import { api, ApiError } from "../api";
import { useAuth } from "../auth";
import { AuthForm } from "../components/AuthForm";
import { Logo } from "../components/Logo";
import { registrationKnownClosed } from "../registration";
import { codeErrorText, codeToSend } from "../twoFactorText";

export function Login() {
  const { state, refresh } = useAuth();
  const navigate = useNavigate();
  const from = (useLocation().state as { from?: string } | null)?.from ?? "/library";
  const [challenge, setChallenge] = useState<string | null>(null);
  if (state.status === "authenticated" && !challenge) {
    return <Navigate to={state.user.mfa_enrol_required ? "/settings/security" : from} replace />;
  }

  if (challenge) {
    return (
      <CodeStep
        challenge={challenge}
        onBack={() => setChallenge(null)}
        onDone={async () => {
          await refresh();
          navigate(from, { replace: true });
        }}
      />
    );
  }

  return (
    <AuthForm
      title="Sign in"
      submitLabel="Sign in"
      passwordAutocomplete="current-password"
      onSubmit={async (email, password) => {
        let res;
        try {
          res = await api.login(email, password);
        } catch (e) {
          if (e instanceof ApiError && e.status === 401) throw new Error("Wrong email or password.", { cause: e });
          if (e instanceof ApiError && e.status === 429) throw new Error("Too many attempts. Wait a minute and try again.", { cause: e });
          throw e;
        }
        if ("mfa_required" in res) {
          // Password right, but the account has a second factor: no session until the code is.
          setChallenge(res.challenge);
          return;
        }
        await refresh();
        navigate(res.mfa_enrol_required ? "/settings/security" : from, { replace: true });
      }}
      footer={
        registrationKnownClosed() ? (
          "Registration is closed on this server."
        ) : (
          <>
            New here? <Link to="/register">Create an account</Link>
          </>
        )
      }
    />
  );
}

/** The second step of a sign-in: the 6-digit code, or a recovery code instead. */
function CodeStep({ challenge, onDone, onBack }: { challenge: string; onDone: () => Promise<void>; onBack: () => void }) {
  const id = useId();
  const [recovery, setRecovery] = useState(false);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.loginCode(challenge, codeToSend(code));
      await onDone();
    } catch (err) {
      const status = err instanceof ApiError ? err.status : 0;
      const c = err instanceof ApiError ? err.code : undefined;
      setError(codeErrorText(status, c, err instanceof Error ? err.message : "Something went wrong."));
      if (c === "challenge_expired") onBack();
      setBusy(false);
    }
  }

  return (
    <div className="auth-page">
      <div className="card auth-card">
        <div className="auth-brand">
          <Logo size={44} />
          <h1>Two-factor code</h1>
        </div>
        <form onSubmit={submit}>
          <label htmlFor={`${id}-code`}>{recovery ? "Recovery code" : "Code from your authenticator app"}</label>
          <input
            id={`${id}-code`}
            name="code"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            autoFocus
            required
            autoComplete="one-time-code"
            inputMode={recovery ? "text" : "numeric"}
            pattern={recovery ? undefined : "[0-9 ]*"}
            maxLength={recovery ? 24 : 7}
            spellCheck={false}
            autoCapitalize="none"
            placeholder={recovery ? "abcd-efgh-2345-ijkl" : "123 456"}
          />
          <div role="alert" className="form-error">
            {error}
          </div>
          <button type="submit" className="btn btn-primary" disabled={busy || code.trim() === ""}>
            {busy ? "Please wait…" : "Sign in"}
          </button>
        </form>
        <p className="auth-footer muted">
          <button
            type="button"
            className="link-button"
            onClick={() => {
              setRecovery(!recovery);
              setCode("");
              setError(null);
            }}
          >
            {recovery ? "Use the code from the app" : "Lost your phone? Use a recovery code"}
          </button>
          {" · "}
          <button type="button" className="link-button" onClick={onBack}>
            Back
          </button>
        </p>
      </div>
    </div>
  );
}
