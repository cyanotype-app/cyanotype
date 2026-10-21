import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import { api, ApiError, type TwoFactorEnrolment, type TwoFactorStatus } from "../api";
import { useAuth } from "../auth";
import { QrCode } from "../components/QrCode";
import { RecoveryCodes } from "../components/RecoveryCodes";
import { cleanCode, codeErrorText, codeToSend, groupSecret } from "../twoFactorText";

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
    <div className="settings">
      <h1>Security</h1>
      {pending && (
        <p role="alert" className="notice">
          The administrator requires two-factor authentication. Set it up below to keep using Cyanotype.
        </p>
      )}
      <h2>Two-factor authentication</h2>
      <p className="muted">
        Sign-in asks for a 6-digit code from an authenticator app on your phone, after your password. Recovery codes are
        the way back in if you lose the phone.
      </p>

      <div role="alert" className="form-error">
        {error}
      </div>

      {codes && <RecoveryCodes codes={codes} email={email} onDone={() => setCodes(null)} />}

      {status === null ? (
        <p className="muted" role="status">
          Loading…
        </p>
      ) : !codes && !status.available ? (
        <p className="notice" role="status">
          Two-factor authentication is not available on this server yet: the administrator has to set it up first.
        </p>
      ) : !codes && !status.enabled && mode === "idle" ? (
        <div>
          <p>
            <strong>Off.</strong> Anyone with your password can sign in.
          </p>
          <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void start()}>
            Set up two-factor authentication
          </button>
        </div>
      ) : null}

      {mode === "enrolling" && enrolment && (
        <Enrol enrolment={enrolment} onCancel={cancel} onConfirmed={(c) => void finished(c)} />
      )}

      {status?.enabled && !codes && (
        <div>
          <p>
            <strong>On.</strong> {status.recovery_codes_remaining} of 10 recovery codes left.
            {status.recovery_codes_remaining <= 3 && " Make new ones soon."}
          </p>
          {mode === "idle" && (
            <div className="actions">
              <button type="button" className="btn" onClick={() => setMode("regenerating")}>
                New recovery codes
              </button>
              <button type="button" className="btn btn-danger-quiet" onClick={() => setMode("disabling")}>
                Turn off
              </button>
            </div>
          )}
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
    </div>
  );
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
  const id = useId();
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
    <section className="enrol" aria-label="Set up two-factor authentication">
      <ol>
        <li>
          <p>Scan this code with an authenticator app (any app that supports one-time codes).</p>
          <QrCode text={enrolment.otpauth_uri} label="QR code for your authenticator app" />
          <p className="muted small">Cannot scan it? Enter this key in the app by hand:</p>
          <code className="secret" aria-label="Setup key">
            {groupSecret(enrolment.secret)}
          </code>
        </li>
        <li>
          <form onSubmit={submit} className="key-form">
            <div>
              <label htmlFor={`${id}-code`}>Then enter the 6-digit code it shows</label>
              <input
                id={`${id}-code`}
                value={code}
                onChange={(e) => setCode(e.target.value)}
                inputMode="numeric"
                autoComplete="one-time-code"
                pattern="[0-9 ]*"
                maxLength={7}
                placeholder="123 456"
                required
              />
            </div>
            <button type="submit" className="btn btn-primary" disabled={busy}>
              Turn on
            </button>
            <button type="button" className="btn btn-quiet" onClick={onCancel}>
              Cancel
            </button>
          </form>
          <div role="alert" className="form-error">
            {error}
          </div>
        </li>
      </ol>
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
  const id = useId();
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
    <form className="security-form" onSubmit={submit} aria-label={title}>
      <h3>{title}</h3>
      {help && <p className="muted small">{help}</p>}
      <label htmlFor={`${id}-pw`}>Password</label>
      <input
        id={`${id}-pw`}
        type="password"
        autoComplete="current-password"
        required
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      <label htmlFor={`${id}-code`}>Code from the app (or a recovery code)</label>
      <input
        id={`${id}-code`}
        value={code}
        onChange={(e) => setCode(e.target.value)}
        autoComplete="one-time-code"
        spellCheck={false}
        required
      />
      <div role="alert" className="form-error">
        {error}
      </div>
      <div className="actions">
        <button type="submit" className={danger ? "btn btn-danger" : "btn btn-primary"} disabled={busy}>
          {submitLabel}
        </button>
        <button type="button" className="btn btn-quiet" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
