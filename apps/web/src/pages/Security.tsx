import { useCallback, useEffect, useState, type FormEvent } from "react";
import { api, ApiError, type TwoFactorEnrolment, type TwoFactorStatus } from "../api";
import { useLocation, useNavigate } from "react-router-dom";
import { useAuth } from "../auth";
import { QrCode } from "../components/QrCode";
import { RecoveryCodes } from "../components/RecoveryCodes";
import { returnToOf } from "../returnTo";
import { cleanCode, codeErrorText, codeToSend, groupSecret } from "../twoFactorText";
import { Field, FormError, inputClass, Loading, Notice, Section } from "../settings/kit";
import { Button } from "../ui/Button";

type Mode = "idle" | "enrolling" | "disabling" | "regenerating";

const messageOf = (e: unknown, fallback: string): string => {
  if (e instanceof ApiError) {
    if (e.code === "wrong_password") return "That password is not right.";
    return codeErrorText(e.status, e.code, e.message || fallback);
  }
  return e instanceof Error ? e.message : fallback;
};

/** Settings > Security: turn on two-factor authentication with an authenticator app, recovery codes, turn it off. */
export function Security() {
  const { state, refresh } = useAuth();
  const email = state.status === "authenticated" ? state.user.email : "";
  const pending = state.status === "authenticated" && state.user.mfa_enrol_required === true;
  // Sent here from a page that needed two-factor authentication first (a command-line login, say): back there once it is set up.
  const returnTo = returnToOf(useLocation().state);
  const navigate = useNavigate();
  const [status, setStatus] = useState<TwoFactorStatus | null>(null);
  const [mode, setMode] = useState<Mode>("idle");
  const [enrolment, setEnrolment] = useState<TwoFactorEnrolment | null>(null);
  const [codes, setCodes] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setStatus(await api.twoFactorStatus());
    } catch (e) {
      setError(messageOf(e, "Could not load the security settings."));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  async function start() {
    setBusy(true);
    setError(null);
    try {
      setEnrolment(await api.twoFactorEnrol());
      setMode("enrolling");
    } catch (e) {
      setError(messageOf(e, "Could not start the setup."));
    } finally {
      setBusy(false);
    }
  }

  function cancel() {
    setMode("idle");
    setEnrolment(null);
    setError(null);
  }

  async function finished(newCodes: string[]) {
    setCodes(newCodes);
    setMode("idle");
    setEnrolment(null);
    await load();
    await refresh();
  }

  return (
    <div className="flex flex-col gap-5">
      {pending && (
        <Notice tone="warn" role="alert">
          The administrator requires two-factor authentication. Set it up below to keep using Cyanotype.
        </Notice>
      )}
      <Section
        title="Two-factor authentication"
        description="Sign-in asks for a 6-digit code from an authenticator app on your phone, after your password. Recovery codes are the way back in if you lose the phone."
      >
        <div className="mb-3">
          <FormError>{error}</FormError>
        </div>

        {codes && (
          <RecoveryCodes
            codes={codes}
            email={email}
            onDone={() => {
              setCodes(null);
              if (returnTo && !pending) navigate(returnTo, { replace: true });
            }}
          />
        )}

        {status === null ? (
          <Loading />
        ) : !codes && !status.available ? (
          <Notice role="status">Two-factor authentication is not available on this server yet: the administrator has to set it up first.</Notice>
        ) : !codes && !status.enabled && mode === "idle" ? (
          <StatusCard>
            <p>
              <strong>Off.</strong> Anyone with your password can sign in.
            </p>
            <Button variant="primary" disabled={busy} onClick={() => void start()}>
              Set up two-factor authentication
            </Button>
          </StatusCard>
        ) : null}

        {mode === "enrolling" && enrolment && <Enrol enrolment={enrolment} onCancel={cancel} onConfirmed={(c) => void finished(c)} />}

        {status?.enabled && !codes && (
          <div className="flex flex-col gap-4">
            <StatusCard>
              <p>
                <strong>On.</strong> {status.recovery_codes_remaining} of 10 recovery codes left.
                {status.recovery_codes_remaining <= 3 && " Make new ones soon."}
              </p>
              {mode === "idle" && (
                <div className="flex flex-wrap gap-2">
                  <Button onClick={() => setMode("regenerating")}>New recovery codes</Button>
                  <Button variant="quiet" className="text-danger" onClick={() => setMode("disabling")}>
                    Turn off
                  </Button>
                </div>
              )}
            </StatusCard>
            {mode === "disabling" && (
              <PasswordAndCode
                title="Turn off two-factor authentication"
                submitLabel="Turn off"
                danger
                onCancel={cancel}
                onSubmit={async (password, code) => {
                  const r = await api.twoFactorDisable(password, code);
                  setMode("idle");
                  await load();
                  await refresh();
                  if (r.enrol_required) setError("The administrator requires it: set it up again to keep using Cyanotype.");
                }}
              />
            )}
            {mode === "regenerating" && (
              <PasswordAndCode
                title="Make new recovery codes"
                help="The old codes stop working."
                submitLabel="Make new codes"
                onCancel={cancel}
                onSubmit={async (password, code) => {
                  const r = await api.newRecoveryCodes(password, code);
                  setMode("idle");
                  setCodes(r.recovery_codes);
                  await load();
                }}
              />
            )}
          </div>
        )}
      </Section>
    </div>
  );
}

function StatusCard({ children }: { children: React.ReactNode }) {
  return <div className="flex flex-col items-start gap-3 rounded-card bg-card p-4 text-[0.9375rem] ring-1 ring-line">{children}</div>;
}

/** Step 1 and 2 of setup: scan (or type the key), then prove it works with a code. */
function Enrol({
  enrolment,
  onCancel,
  onConfirmed,
}: {
  enrolment: TwoFactorEnrolment;
  onCancel: () => void;
  onConfirmed: (codes: string[]) => void;
}) {
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!cleanCode(code)) {
      setError("Enter the 6 digits shown in the app.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const r = await api.twoFactorConfirm(cleanCode(code) ?? "");
      onConfirmed(r.recovery_codes);
    } catch (err) {
      setError(messageOf(err, "Could not confirm the code."));
      setBusy(false);
    }
  }

  return (
    <section aria-label="Set up two-factor authentication" className="grid grid-cols-1 gap-6 rounded-card bg-card p-4 ring-1 ring-line md:grid-cols-[auto_minmax(0,1fr)] md:p-5">
      <div className="flex flex-col items-center gap-2 md:items-start">
        <p className="text-sm font-medium">1. Scan this code with an authenticator app</p>
        <QrCode text={enrolment.otpauth_uri} label="QR code for your authenticator app" />
      </div>
      <div className="flex min-w-0 flex-col gap-4">
        <div>
          <p className="text-[0.8125rem] text-muted">Cannot scan it? Enter this key in the app by hand:</p>
          <code
            className="mt-1.5 block rounded-xl bg-surface px-3 py-2.5 font-mono text-[0.8125rem] tracking-wide break-words"
            aria-label="Setup key"
          >
            {groupSecret(enrolment.secret)}
          </code>
        </div>
        <form onSubmit={submit} className="flex flex-col gap-3">
          <Field label="Then enter the 6-digit code it shows">
            {(p) => (
              <input
                {...p}
                className={inputClass}
                value={code}
                onChange={(e) => setCode(e.target.value)}
                inputMode="numeric"
                autoComplete="one-time-code"
                pattern="[0-9 ]*"
                maxLength={7}
                placeholder="123 456"
                required
              />
            )}
          </Field>
          <FormError>{error}</FormError>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" variant="primary" disabled={busy}>
              Turn on
            </Button>
            <Button variant="quiet" onClick={onCancel}>
              Cancel
            </Button>
          </div>
        </form>
      </div>
    </section>
  );
}

/** Password and a code: what turning 2FA off or remaking recovery codes asks for. */
function PasswordAndCode({
  title,
  help,
  submitLabel,
  danger,
  onSubmit,
  onCancel,
}: {
  title: string;
  help?: string;
  submitLabel: string;
  danger?: boolean;
  onSubmit: (password: string, code: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await onSubmit(password, codeToSend(code));
    } catch (err) {
      setError(messageOf(err, "Something went wrong."));
      setBusy(false);
    }
  }

  return (
    <form className="flex flex-col gap-4 rounded-card bg-card p-4 ring-1 ring-line" onSubmit={submit} aria-label={title}>
      <div>
        <h3 className="text-[1.0625rem] font-semibold tracking-tight">{title}</h3>
        {help && <p className="mt-0.5 text-sm text-muted">{help}</p>}
      </div>
      <Field label="Password">
        {(p) => (
          <input {...p} className={inputClass} type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
        )}
      </Field>
      <Field label="Code from the app (or a recovery code)">
        {(p) => (
          <input {...p} className={inputClass} value={code} onChange={(e) => setCode(e.target.value)} autoComplete="one-time-code" spellCheck={false} required />
        )}
      </Field>
      <FormError>{error}</FormError>
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant={danger ? "danger" : "primary"} disabled={busy}>
          {submitLabel}
        </Button>
        <Button variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
