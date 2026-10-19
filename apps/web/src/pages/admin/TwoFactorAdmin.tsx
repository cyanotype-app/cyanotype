import { useCallback, useEffect, useState } from "react";
import { api, type TwoFactorAccount, type TwoFactorOverview } from "../../api";
import { useAuth } from "../../auth";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleDateString() : "");

/** Admin > Security: the server key, requiring 2FA for everyone, who has it on, and resetting someone's. */
export function TwoFactorAdmin() {
  const { state } = useAuth();
  const me = state.status === "authenticated" ? state.user.id : "";
  const [data, setData] = useState<TwoFactorOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setData(await api.adminTwoFactor());
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load the two-factor overview.");
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  async function setRequired(required: boolean) {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await api.adminRequireTwoFactor(required);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not change the setting.");
    } finally {
      setBusy(false);
    }
  }

  async function reset(u: TwoFactorAccount) {
    if (
      !window.confirm(
        `Reset two-factor authentication for ${u.email}? They are signed out everywhere and can sign in with their password alone. This is written to the audit log.`,
      )
    )
      return;
    setError(null);
    setNotice(null);
    try {
      await api.adminResetTwoFactor(u.id);
      setNotice(`Reset for ${u.email}.`);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not reset.");
    }
  }

  if (!data) {
    return (
      <p className="muted" role="status">
        {error ?? "Loading…"}
      </p>
    );
  }
  const on = data.users.filter((u) => u.enabled).length;
  return (
    <div className="two-factor-admin">
      <h2>Two-factor authentication</h2>
      {!data.key_configured && (
        <p role="alert" className="notice warn">
          <strong>Two-factor authentication is switched off on this server.</strong> No server key is set, so nobody can
          set it up, and accounts that already have it cannot sign in. Set <code>AUTH_TOTP_KEY</code> (see the upgrade
          notes in docs/deploy.md) and restart the auth service.
        </p>
      )}
      <div role="alert" className="form-error">
        {error}
      </div>
      {notice && <p role="status">{notice}</p>}

      <section className="require">
        <label className="check">
          <input
            type="checkbox"
            checked={data.required}
            disabled={busy || (!data.key_configured && !data.required)}
            onChange={(e) => void setRequired(e.target.checked)}
          />{" "}
          Require two-factor authentication for every account
        </label>
        <p className="muted small">
          Accounts without it are sent to set it up right after they sign in, and can do nothing else until they have. You
          need it on your own account first.
        </p>
        {data.required && !data.key_configured && (
          <p className="muted small">Not enforced right now: this server has no key.</p>
        )}
      </section>

      <h3>
        Accounts ({on} of {data.users.length} have it on)
      </h3>
      <ul className="key-list tfa-list">
        {data.users.map((u) => (
          <li key={u.id}>
            <div>
              <strong>{u.email}</strong> {u.role === "admin" && <span className="muted small">(admin)</span>}
              <div className="muted small">
                {u.enabled
                  ? `On since ${when(u.since)} · ${u.recovery_codes_remaining} recovery codes left`
                  : "Off"}
              </div>
            </div>
            {u.enabled && u.id !== me && (
              <button type="button" className="btn btn-quiet" onClick={() => void reset(u)}>
                Reset
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
