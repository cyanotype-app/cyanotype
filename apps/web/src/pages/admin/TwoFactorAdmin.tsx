import { useCallback, useEffect, useState } from "react";
import { api, type TwoFactorAccount, type TwoFactorOverview } from "../../api";
import { useAuth } from "../../auth";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { FormError, Group, Loading, Notice, Row, Section, StatusNote } from "../../settings/kit";
import { Button } from "../../ui/Button";
import { Switch } from "../../ui/Controls";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleDateString() : "");

/** Admin > Security: the server key, requiring 2FA for everyone, who has it on, and resetting someone's. */
export function TwoFactorAdmin() {
  const { state } = useAuth();
  const me = state.status === "authenticated" ? state.user.id : "";
  const [data, setData] = useState<TwoFactorOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [resetting, setResetting] = useState<TwoFactorAccount | null>(null);

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
    setResetting(null);
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
    return <Loading>{error ?? "Loading…"}</Loading>;
  }
  const on = data.users.filter((u) => u.enabled).length;
  return (
    <div className="flex flex-col gap-5">
      {!data.key_configured && (
        <Notice tone="warn" role="alert">
          <strong>Two-factor authentication is switched off on this server.</strong> No server key is set, so nobody can set it up, and
          accounts that already have it cannot sign in. Set <code className="font-mono text-[0.8125rem]">AUTH_TOTP_KEY</code> (see the upgrade notes in
          docs/deploy.md) and restart the auth service.
        </Notice>
      )}
      <FormError>{error}</FormError>
      <StatusNote>{notice}</StatusNote>

      <Section>
        <Group>
          <div className="px-4 py-3">
            <Switch
              label="Require two-factor authentication for every account"
              hint="Accounts without it are sent to set it up right after they sign in, and can do nothing else until they have. You need it on your own account first."
              checked={data.required}
              disabled={busy || (!data.key_configured && !data.required)}
              onChange={(on) => void setRequired(on)}
            />
            {data.required && !data.key_configured && <p className="mt-1 text-[0.8125rem] text-muted">Not enforced right now: this server has no key.</p>}
          </div>
        </Group>
      </Section>

      <Section title={`Accounts (${on} of ${data.users.length} have it on)`}>
        <Group as="ul">
          {data.users.map((u) => (
            <Row
              as="li"
              key={u.id}
              label={
                <>
                  <strong className="font-medium">{u.email}</strong> {u.role === "admin" && <span className="text-[0.8125rem] text-muted">(admin)</span>}
                </>
              }
              detail={u.enabled ? `On since ${when(u.since)} · ${u.recovery_codes_remaining} recovery codes left` : "Off"}
            >
              {u.enabled && u.id !== me && (
                <Button size="sm" onClick={() => setResetting(u)}>
                  Reset
                </Button>
              )}
            </Row>
          ))}
        </Group>
      </Section>

      {resetting && (
        <ConfirmDialog
          title={`Reset two-factor authentication for ${resetting.email}?`}
          confirmLabel="Reset"
          danger
          onCancel={() => setResetting(null)}
          onConfirm={() => void reset(resetting)}
        >
          <p>They are signed out everywhere and can sign in with their password alone. This is written to the audit log.</p>
        </ConfirmDialog>
      )}
    </div>
  );
}
