import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import { api, type ApiKey, type CreatedApiKey } from "../api";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "never");

/** Create, copy (shown once), list and revoke API keys, e.g. for `cyanotype login --api-key`. */
export function ApiKeys() {
  const nameId = useId();
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
  const [name, setName] = useState("");
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
      setCreated(await api.createApiKey(name.trim()));
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
        Keys let the <code>cyanotype</code> command line import photos without your password. Run{" "}
        <code>cyanotype login --server {window.location.origin} --api-key …</code>.
      </p>

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
        <button type="submit" className="btn btn-primary" disabled={busy || name.trim() === ""}>
          Create key
        </button>
      </form>

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
                  Created {when(k.created_at)} · last used {when(k.last_used_at)}
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
