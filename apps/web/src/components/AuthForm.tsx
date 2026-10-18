import { useId, useState, type FormEvent, type ReactNode } from "react";
import { Logo } from "./Logo";

interface Props {
  title: string;
  submitLabel: string;
  passwordAutocomplete: "current-password" | "new-password";
  /** Rejects with an Error whose message is shown to the user. */
  onSubmit: (email: string, password: string) => Promise<void>;
  footer: ReactNode;
  /** Shown above the form (e.g. "registration is closed"). */
  notice?: ReactNode;
  hint?: string;
  /** More fields, after the password (e.g. the invite code). */
  extra?: ReactNode;
}

export function AuthForm({ title, submitLabel, passwordAutocomplete, onSubmit, footer, notice, hint, extra }: Props) {
  const id = useId();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await onSubmit(email, password);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Something went wrong.");
      setBusy(false);
    }
  }

  return (
    <div className="auth-page">
      <div className="card auth-card">
        <div className="auth-brand">
          <Logo size={44} />
          <h1>{title}</h1>
        </div>
        {notice}
        <form onSubmit={submit} noValidate={false}>
          <label htmlFor={`${id}-email`}>Email</label>
          <input
            id={`${id}-email`}
            type="email"
            name="email"
            autoComplete="username"
            required
            autoFocus
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
          <label htmlFor={`${id}-password`}>Password</label>
          <input
            id={`${id}-password`}
            type="password"
            name="password"
            autoComplete={passwordAutocomplete}
            required
            minLength={passwordAutocomplete === "new-password" ? 8 : undefined}
            aria-describedby={hint ? `${id}-hint` : undefined}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          {hint && (
            <p id={`${id}-hint`} className="muted small">
              {hint}
            </p>
          )}
          {extra}
          <div role="alert" className="form-error">
            {error}
          </div>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            {busy ? "Please wait…" : submitLabel}
          </button>
        </form>
        <p className="auth-footer muted">{footer}</p>
      </div>
    </div>
  );
}
