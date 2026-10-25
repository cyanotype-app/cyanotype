import type { ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import type { ApiKeyScope } from "../api";
import { useAuth } from "../auth";
import { ALLOW_QUESTION, ONLY_IF_YOU_STARTED_IT } from "../cliLoginText";
import { SCOPE_HELP, SCOPE_LABEL } from "../twoFactorText";
import { Logo } from "./Logo";

/** The pages that approve a command-line login sit outside the app shell. Anonymous visitors sign in first and come back here. */
export function CliPage({ title, children }: { title: string; children: (email: string) => ReactNode }) {
  const { state } = useAuth();
  const loc = useLocation();
  if (state.status === "loading")
    return (
      <p className="center muted" role="status">
        Loading…
      </p>
    );
  if (state.status === "anonymous") {
    return <Navigate to="/login" replace state={{ from: loc.pathname + loc.search }} />;
  }
  if (state.user.mfa_enrol_required) return <Navigate to="/settings/security" replace />;
  return (
    <div className="auth-page">
      <div className="card auth-card cli-card">
        <div className="auth-brand">
          <Logo size={44} />
          <h1>{title}</h1>
        </div>
        {children(state.user.email)}
      </div>
    </div>
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
    <section aria-label="Allow the command line">
      <p className="cli-question">{ALLOW_QUESTION(device)}</p>
      <dl className="cli-facts">
        <dt>Device</dt>
        <dd>{device}</dd>
        <dt>Access</dt>
        <dd>
          <strong>{SCOPE_LABEL[scope] ?? scope}</strong>
          <span className="muted small"> {SCOPE_HELP[scope] ?? ""}</span>
        </dd>
        <dt>Account</dt>
        <dd>{email}</dd>
      </dl>
      <p className="notice">{ONLY_IF_YOU_STARTED_IT}</p>
      <div className="cli-actions">
        <button type="button" className="btn btn-primary" disabled={busy} onClick={onAllow}>
          Allow
        </button>
        <button type="button" className="btn btn-quiet" disabled={busy} onClick={onDeny}>
          Deny
        </button>
      </div>
      <div role="alert" className="form-error">
        {error}
      </div>
    </section>
  );
}
