import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import { api, type ApiKey, type ApiKeyScope, type CreatedApiKey } from "../api";
import { EXPIRY_CHOICES, expiryLine, SCOPE_HELP, SCOPE_LABEL } from "../twoFactorText";
import { detectOs, installCommands, OS_LABEL, type CliOs } from "../cliInstall";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "never");

/** The one-line install of the command line from this server, for the visitor's OS first. */
function InstallCli() {
  const cmds = installCommands(window.location.origin);
  const mine = detectOs(navigator.userAgent);
  const order: CliOs[] = mine === "windows" ? ["windows", "unix"] : ["unix", "windows"];
  return (
    <section className="install-cli" aria-label="Install the command line">
      <h2>Install the command line</h2>
      {order.map((os, i) => (
        <div key={os} className={i === 0 ? undefined : "muted"}>
          <div className="small">{OS_LABEL[os]}</div>
          <code>{cmds[os]}</code>
        </div>
      ))}
      <p className="muted small">
        Then run <code>cyanotype login {window.location.origin}</code>. Later, <code>cyanotype update</code> keeps it
        current.
      </p>
    </section>
  );
}

/** Create, copy (shown once), list and revoke API keys, e.g. for `cyanotype login --api-key`. */
export function ApiKeys() {
  const nameId = useId();
  const scopeId = useId();
  const expiryId = useId();
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<ApiKeyScope>("import");
  const [expiry, setExpiry] = useState<number | null>(null);
  const [created, setCreated] = useState<CreatedApiKey | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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
    if (!window.confirm(`Revoke "${k.name}"? Anything using it stops working.`)) return;
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
    <div className="settings">
      <h1>API keys</h1>
      <p className="muted">
        Keys let the <code>cyanotype</code> command line import photos without your password. The easy way is{" "}
        <code>cyanotype login {window.location.origin}</code>, which asks you to allow it here and adds a key named
        &quot;cyanotype CLI on …&quot; below. To use a key you made yourself, run{" "}
        <code>cyanotype login {window.location.origin} --api-key …</code>.
      </p>

      <InstallCli />

      <form className="key-form" onSubmit={create}>
        <div>
          <label htmlFor={nameId}>Name</label>
          <input
            id={nameId}
            value={name}
            maxLength={100}
            required
            placeholder="e.g. laptop"
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <div>
          <label htmlFor={scopeId}>Can do</label>
          <select id={scopeId} value={scope} onChange={(e) => setScope(e.target.value as ApiKeyScope)}>
            {(Object.keys(SCOPE_LABEL) as ApiKeyScope[]).map((s) => (
              <option key={s} value={s}>
                {SCOPE_LABEL[s]}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={expiryId}>Expires</label>
          <select
            id={expiryId}
            value={expiry === null ? "" : String(expiry)}
            onChange={(e) => setExpiry(e.target.value === "" ? null : Number(e.target.value))}
          >
            {EXPIRY_CHOICES.map((c) => (
              <option key={c.label} value={c.days === null ? "" : String(c.days)}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
        <button type="submit" className="btn btn-primary" disabled={busy || name.trim() === ""}>
          Create key
        </button>
      </form>
      <p className="muted small">{SCOPE_HELP[scope]}</p>

      <div role="alert" className="form-error">
        {error}
      </div>

      {created && (
        <section className="new-key" aria-label="New API key">
          <strong>Copy your new key now. It will not be shown again.</strong>
          <code>{created.key}</code>
          <button type="button" className="btn btn-primary" onClick={() => void copy()}>
            {copied ? "Copied" : "Copy"}
          </button>{" "}
          <button type="button" className="btn btn-quiet" onClick={() => setCreated(null)}>
            Done
          </button>
        </section>
      )}

      {keys === null ? (
        <p className="muted" role="status">
          Loading…
        </p>
      ) : keys.length === 0 ? (
        <p className="muted">No API keys yet.</p>
      ) : (
        <ul className="key-list">
          {keys.map((k) => (
            <li key={k.id}>
              <div>
                <strong>{k.name}</strong> <code>{k.prefix}…</code>
                <div className="muted small">
                  {SCOPE_LABEL[k.scope] ?? k.scope} · {expiryLine(k)} · created {when(k.created_at)} · last used{" "}
                  {when(k.last_used_at)}
                </div>
              </div>
              <button type="button" className="btn btn-quiet" onClick={() => void revoke(k)}>
                Revoke
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
