import { useEffect, useState } from "react";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { api, ApiError, type RegistrationInfo } from "../api";
import { useAuth } from "../auth";
import { AuthForm } from "../components/AuthForm";
import { Field, inputClass, Notice } from "../settings/kit";
import { markRegistrationClosed, parseInviteToken, registrationKnownClosed } from "../registration";

type InviteState = "unknown" | "checking" | "valid" | "used" | "revoked" | "expired" | "invalid";

const INVITE_PROBLEM: Record<string, string> = {
  used: "This invite link was already used. Ask for a new one.",
  revoked: "This invite link was revoked. Ask for a new one.",
  expired: "This invite link has expired. Ask for a new one.",
  invalid: "This invite link is not valid. Check that you copied all of it, or ask for a new one.",
};

export function Register() {
  const { state, refresh } = useAuth();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const linked = parseInviteToken(params.get("invite") ?? "");
  const [pasted, setPasted] = useState("");
  const [closed, setClosed] = useState(registrationKnownClosed());
  const [info, setInfo] = useState<RegistrationInfo | null>(null);
  const [inviteState, setInviteState] = useState<InviteState>(linked ? "checking" : "unknown");
  const token = linked || parseInviteToken(pasted);

  useEffect(() => {
    let live = true;
    api
      .registrationInfo()
      .then((i) => live && setInfo(i))
      .catch(() => undefined); // an older server: fall back to learning from the 403
    return () => {
      live = false;
    };
  }, []);

  // A link in the address bar is checked up front, so a spent or expired one says so before the form is filled in.
  useEffect(() => {
    if (!linked) return;
    let live = true;
    api
      .checkInvite(linked)
      .then((r) => live && setInviteState(r.valid ? "valid" : r.reason === "unknown" || !r.reason ? "invalid" : r.reason))
      .catch(() => live && setInviteState("unknown"));
    return () => {
      live = false;
    };
  }, [linked]);

  if (state.status === "authenticated") return <Navigate to="/" replace />;

  const needsInvite = info?.invite_required === true && !linked;
  const refused = closed || (info !== null && !info.open && !info.invite_required);
  const problem = inviteState in INVITE_PROBLEM ? INVITE_PROBLEM[inviteState] : null;

  return (
    <AuthForm
      title="Create account"
      submitLabel="Create account"
      passwordAutocomplete="new-password"
      hint="At least 8 characters."
      notice={
        <>
          {info?.first_account && info.open && (
            <Notice role="status">This server has no accounts yet. The first one becomes the administrator.</Notice>
          )}
          {problem && (
            <Notice tone="warn" role="alert">
              {problem}
            </Notice>
          )}
          {inviteState === "valid" && (
            <Notice tone="ok" role="status">
              You have been invited. Create your account below.
            </Notice>
          )}
          {refused && !problem && (
            <Notice role="status">
              {info?.mode === "first_user" || info === null
                ? "Registration is closed on this server. Ask the owner to add you, or sign in if you already have an account."
                : "Registration is closed on this server. Sign in if you already have an account."}
            </Notice>
          )}
        </>
      }
      extra={
        needsInvite && (
          <Field label="Invite link or code" hint="This server is invite-only. Ask its administrator for a link.">
            {(p) => (
              <input
                {...p}
                className={inputClass}
                name="invite"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                required
                value={pasted}
                placeholder="Paste the link you were sent"
                onChange={(e) => setPasted(e.target.value)}
              />
            )}
          </Field>
        )
      }
      onSubmit={async (email, password) => {
        try {
          await api.register(email, password, token || undefined);
        } catch (e) {
          if (e instanceof ApiError && e.status === 403) {
            // An invite problem has its own message; anything else means the door is shut.
            if (/invite/i.test(e.message)) throw new Error(sentence(e.message), { cause: e });
            markRegistrationClosed();
            setClosed(true);
            throw new Error("Registration is closed.", { cause: e });
          }
          if (e instanceof ApiError && e.status === 409) throw new Error("That email is already registered.", { cause: e });
          if (e instanceof ApiError && e.status === 429) throw new Error("Too many attempts. Wait a minute and try again.", { cause: e });
          throw e;
        }
        await refresh();
        navigate("/", { replace: true });
      }}
      footer={
        <>
          Already have an account? <Link to="/login">Sign in</Link>
        </>
      }
    />
  );
}

/** "this invite link was revoked" as a sentence for the form's error line. */
const sentence = (m: string): string => m.charAt(0).toUpperCase() + m.slice(1) + (m.endsWith(".") ? "" : ".");
