import { useState, type FormEvent, type ReactNode } from "react";
import { Field, FormError, inputClass } from "../settings/kit";
import { Button } from "../ui/Button";
import { AuthShell } from "./AuthShell";

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
    <AuthShell title={title} footer={footer}>
      {notice && <div className="mb-5 flex flex-col gap-3">{notice}</div>}
      <form onSubmit={submit} className="flex flex-col gap-4">
        <Field label="Email">
          {(p) => (
            <input
              {...p}
              className={inputClass}
              type="email"
              name="email"
              autoComplete="username"
              required
              autoFocus
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          )}
        </Field>
        <Field label="Password" hint={hint}>
          {(p) => (
            <input
              {...p}
              className={inputClass}
              type="password"
              name="password"
              autoComplete={passwordAutocomplete}
              required
              minLength={passwordAutocomplete === "new-password" ? 8 : undefined}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          )}
        </Field>
        {extra}
        <FormError>{error}</FormError>
        <Button type="submit" variant="primary" size="lg" className="w-full" disabled={busy}>
          {busy ? "Please wait…" : submitLabel}
        </Button>
      </form>
    </AuthShell>
  );
}
