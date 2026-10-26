import type { ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import type { ApiKeyScope } from "../api";
import { useAuth } from "../auth";
import { ALLOW_QUESTION, ONLY_IF_YOU_STARTED_IT } from "../cliLoginText";
import { SCOPE_HELP, SCOPE_LABEL } from "../twoFactorText";
import { FormError, Loading, Notice } from "../settings/kit";
import { Button } from "../ui/Button";
import { AuthShell } from "./AuthShell";

/** The pages that approve a command-line login sit outside the app shell. Anonymous visitors sign in (and, when the server requires it, set up two-factor authentication) first and come back here. */
export function CliPage({ title, children }: { title: string; children: (email: string) => ReactNode }) {
  const { state } = useAuth();
  const loc = useLocation();
  if (state.status === "loading")
    return (
      <main className="flex min-h-dvh items-center justify-center bg-bg">
        <Loading />
      </main>
    );
  // The hash is kept: /activate#code=... must still have its code when the person is back from signing in.
  const here = loc.pathname + loc.search + loc.hash;
  if (state.status === "anonymous") {
    return <Navigate to="/login" replace state={{ from: here }} />;
  }
  // The administrator requires two-factor authentication and this account has none: enrol, then come back here.
  if (state.user.mfa_enrol_required) return <Navigate to="/settings/security" replace state={{ from: here }} />;
  return (
    <AuthShell title={title} wide>
      {children(state.user.email)}
    </AuthShell>
  );
}

interface ConsentProps {
  device: string;
  scope: ApiKeyScope;
  email: string;
  busy: boolean;
  error: string | null;
  onAllow: () => void;
  onDeny: () => void;
}

/** The question itself: which device, what it may do, who is answering, and the two buttons. */
export function CliConsent({ device, scope, email, busy, error, onAllow, onDeny }: ConsentProps) {
  return (
    <section aria-label="Allow the command line" className="flex flex-col gap-5">
      <p className="text-[1.0625rem] leading-snug font-semibold">{ALLOW_QUESTION(device)}</p>
      <dl className="divide-y divide-line overflow-hidden rounded-card bg-surface text-[0.9375rem]">
        <div className="flex flex-col gap-0.5 px-4 py-2.5 min-[420px]:flex-row min-[420px]:gap-4">
          <dt className="w-20 shrink-0 text-muted">Device</dt>
          <dd className="min-w-0 break-words">{device}</dd>
        </div>
        <div className="flex flex-col gap-0.5 px-4 py-2.5 min-[420px]:flex-row min-[420px]:gap-4">
          <dt className="w-20 shrink-0 text-muted">Access</dt>
          <dd className="min-w-0">
            <strong className="font-semibold">{SCOPE_LABEL[scope] ?? scope}</strong>
            <span className="block text-[0.8125rem] text-muted">{SCOPE_HELP[scope] ?? ""}</span>
          </dd>
        </div>
        <div className="flex flex-col gap-0.5 px-4 py-2.5 min-[420px]:flex-row min-[420px]:gap-4">
          <dt className="w-20 shrink-0 text-muted">Account</dt>
          <dd className="min-w-0 break-all">{email}</dd>
        </div>
      </dl>
      <Notice>{ONLY_IF_YOU_STARTED_IT}</Notice>
      <div className="flex flex-col gap-2">
        <Button variant="primary" size="lg" disabled={busy} onClick={onAllow}>
          Allow
        </Button>
        <Button size="lg" disabled={busy} onClick={onDeny}>
          Deny
        </Button>
      </div>
      <FormError>{error}</FormError>
    </section>
  );
}
