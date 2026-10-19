import { useId, useState, type FormEvent } from "react";
import type { Limits } from "../api";
import { gigabytesToBytes } from "../adminText";
import { gigabytesField } from "../limitsText";

interface Props {
  initial: Partial<Limits>;
  /** Rejects with an Error whose message is shown. */
  onSave: (limits: Limits) => Promise<void>;
  onCancel: () => void;
  saveLabel?: string;
}

/** Three limit fields in gigabytes; an empty field means no limit. */
export function LimitsForm({ initial, onSave, onCancel, saveLabel = "Save limits" }: Props) {
  const id = useId();
  const [quota, setQuota] = useState(gigabytesField(initial.quota_bytes));
  const [daily, setDaily] = useState(gigabytesField(initial.daily_upload_bytes));
  const [maxFile, setMaxFile] = useState(gigabytesField(initial.max_file_bytes));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const q = gigabytesToBytes(quota);
    const d = gigabytesToBytes(daily);
    const f = gigabytesToBytes(maxFile);
    if (q === undefined || d === undefined || f === undefined) {
      setError("Limits are amounts in GB greater than zero, such as 5 or 0.5. Leave a field empty for no limit.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSave({ quota_bytes: q, daily_upload_bytes: d, max_file_bytes: f });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save the limits.");
      setBusy(false);
    }
  }

  return (
    <form className="limits-form" onSubmit={submit}>
      <div className="invite-limit-fields">
        <div>
          <label htmlFor={`${id}-q`}>Storage (GB)</label>
          <input id={`${id}-q`} inputMode="decimal" value={quota} placeholder="No limit" onChange={(e) => setQuota(e.target.value)} />
        </div>
        <div>
          <label htmlFor={`${id}-d`}>Uploads per day (GB)</label>
          <input id={`${id}-d`} inputMode="decimal" value={daily} placeholder="No limit" onChange={(e) => setDaily(e.target.value)} />
        </div>
        <div>
          <label htmlFor={`${id}-f`}>Largest file (GB)</label>
          <input id={`${id}-f`} inputMode="decimal" value={maxFile} placeholder="No limit" onChange={(e) => setMaxFile(e.target.value)} />
        </div>
      </div>
      <div role="alert" className="form-error">
        {error}
      </div>
      <div className="invite-actions">
        <button type="submit" className="btn btn-primary" disabled={busy}>
          {saveLabel}
        </button>
        <button type="button" className="btn btn-quiet" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
