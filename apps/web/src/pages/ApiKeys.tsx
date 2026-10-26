import { useCallback, useEffect, useState, type FormEvent } from "react";
import { api, type ApiKey, type ApiKeyScope, type CreatedApiKey } from "../api";
import { EXPIRY_CHOICES, expiryLine, SCOPE_HELP, SCOPE_LABEL } from "../twoFactorText";
import { detectOs, installCommands, OS_LABEL, type CliOs } from "../cliInstall";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Block, Field, FormError, Group, inputClass, Loading, Mono, Row, Section, SecretBox, SelectBox } from "../settings/kit";
import { Button } from "../ui/Button";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "never");

/** The one-line install of the command line from this server, for the visitor's OS first. */
function InstallCli() {
  const cmds = installCommands(window.location.origin);
  const mine = detectOs(navigator.userAgent);
  const order: CliOs[] = mine === "windows" ? ["windows", "unix"] : ["unix", "windows"];
  return (
    <Section title="Install the command line" label="Install the command line">
      <Group>
        {order.map((os, i) => (
          <Block key={os}>
            <p className={i === 0 ? "text-sm font-medium" : "text-sm text-muted"}>{OS_LABEL[os]}</p>
            <code className="mt-1.5 block overflow-x-auto rounded-xl bg-surface p-3 font-mono text-[0.8125rem] break-all">{cmds[os]}</code>
          </Block>
        ))}
      </Group>
      <p className="mt-2 text-[0.8125rem] text-muted">
        Then run <Mono>cyanotype login {window.location.origin}</Mono>. Later, <Mono>cyanotype update</Mono> keeps it current.
      </p>
    </Section>
  );
}

/** Create, copy (shown once), list and revoke API keys, e.g. for `cyanotype login --api-key`. */
export function ApiKeys() {
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<ApiKeyScope>("import");
  const [expiry, setExpiry] = useState<number | null>(null);
  const [created, setCreated] = useState<CreatedApiKey | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [revoking, setRevoking] = useState<ApiKey | null>(null);

  const load = useCallback(async () => {
    try {
      setKeys((await api.listApiKeys()).items);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load keys.");
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  async function create(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setCreated(await api.createApiKey(name.trim(), scope, expiry ?? undefined));
      setCopied(false);
      setName("");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not create the key.");
    } finally {
      setBusy(false);
    }
  }

  async function revoke(k: ApiKey) {
    setRevoking(null);
    setError(null);
    try {
      await api.revokeApiKey(k.id);
      if (created?.id === k.id) setCreated(null);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not revoke the key.");
    }
  }

  async function copy() {
    if (!created) return;
    try {
      await navigator.clipboard.writeText(created.key);
      setCopied(true);
    } catch {
      setError("Could not copy automatically. Select the key and copy it by hand.");
    }
  }

  return (
    <div>
      <p className="text-sm text-muted">
        Keys let the <Mono>cyanotype</Mono> command line import photos without your password. The easy way is{" "}
        <Mono>cyanotype login {window.location.origin}</Mono>, which asks you to allow it here and adds a key named
        &quot;cyanotype CLI on …&quot; below. To use a key you made yourself, run{" "}
        <Mono>cyanotype login {window.location.origin} --api-key …</Mono>.
      </p>

      <InstallCli />

      <Section title="New key">
        <form onSubmit={create} className="flex flex-col gap-4 rounded-card bg-card p-4 ring-1 ring-line">
          <Field label="Name">
            {(p) => (
              <input
                {...p}
                className={inputClass}
                value={name}
                maxLength={100}
                required
                placeholder="e.g. laptop"
                onChange={(e) => setName(e.target.value)}
              />
            )}
          </Field>
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label="Can do" hint={SCOPE_HELP[scope]}>
              {(p) => (
                <SelectBox {...p} value={scope} onChange={(e) => setScope(e.target.value as ApiKeyScope)}>
                  {(Object.keys(SCOPE_LABEL) as ApiKeyScope[]).map((s) => (
                    <option key={s} value={s}>
                      {SCOPE_LABEL[s]}
                    </option>
                  ))}
                </SelectBox>
              )}
            </Field>
            <Field label="Expires">
              {(p) => (
                <SelectBox
                  {...p}
                  value={expiry === null ? "" : String(expiry)}
                  onChange={(e) => setExpiry(e.target.value === "" ? null : Number(e.target.value))}
                >
                  {EXPIRY_CHOICES.map((c) => (
                    <option key={c.label} value={c.days === null ? "" : String(c.days)}>
                      {c.label}
                    </option>
                  ))}
                </SelectBox>
              )}
            </Field>
          </div>
          <FormError>{error}</FormError>
          <div>
            <Button type="submit" variant="primary" disabled={busy || name.trim() === ""}>
              Create key
            </Button>
          </div>
        </form>
      </Section>

      {created && (
        <Section>
          <SecretBox
            label="New API key"
            title="Copy your new key now. It will not be shown again."
            actions={
              <>
                <Button variant="primary" onClick={() => void copy()}>
                  {copied ? "Copied" : "Copy"}
                </Button>
                <Button variant="quiet" onClick={() => setCreated(null)}>
                  Done
                </Button>
              </>
            }
          >
            {created.key}
          </SecretBox>
        </Section>
      )}

      <Section title="Your keys">
        {keys === null ? (
          <Loading />
        ) : keys.length === 0 ? (
          <p className="text-sm text-muted">No API keys yet.</p>
        ) : (
          <Group as="ul">
            {keys.map((k) => (
              <Row
                as="li"
                key={k.id}
                label={
                  <>
                    <strong className="font-medium">{k.name}</strong> <code className="font-mono text-[0.8125rem] text-muted">{k.prefix}…</code>
                  </>
                }
                detail={
                  <>
                    {SCOPE_LABEL[k.scope] ?? k.scope} · {expiryLine(k)} · created {when(k.created_at)} · last used {when(k.last_used_at)}
                  </>
                }
              >
                <Button size="sm" onClick={() => setRevoking(k)}>
                  Revoke
                </Button>
              </Row>
            ))}
          </Group>
        )}
      </Section>

      {revoking && (
        <ConfirmDialog
          title={`Revoke “${revoking.name}”?`}
          confirmLabel="Revoke"
          danger
          onCancel={() => setRevoking(null)}
          onConfirm={() => void revoke(revoking)}
        >
          <p>Anything using it stops working.</p>
        </ConfirmDialog>
      )}
    </div>
  );
}
