import { useState, type FormEvent } from "react";
import type { Limits } from "../api";
import { gigabytesToBytes } from "../adminText";
import { gigabytesField } from "../limitsText";
import { Field, FormError, inputClass } from "../settings/kit";
import { Button } from "../ui/Button";

interface Props {
  initial: Partial<Limits>;
  /** Rejects with an Error whose message is shown. */
  onSave: (limits: Limits) => Promise<void>;
  onCancel: () => void;
  saveLabel?: string;
}

/** Three limit fields in gigabytes; an empty field means no limit. */
export function LimitsForm({ initial, onSave, onCancel, saveLabel = "Save limits" }: Props) {
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

  const field = (label: string, value: string, set: (v: string) => void) => (
    <Field label={label}>
      {(p) => <input {...p} className={inputClass} inputMode="decimal" value={value} placeholder="No limit" onChange={(e) => set(e.target.value)} />}
    </Field>
  );

  return (
    <form className="flex flex-col gap-4" onSubmit={submit}>
      <div className="grid grid-cols-1 gap-3 min-[480px]:grid-cols-3">
        {field("Storage (GB)", quota, setQuota)}
        {field("Uploads per day (GB)", daily, setDaily)}
        {field("Largest file (GB)", maxFile, setMaxFile)}
      </div>
      <FormError>{error}</FormError>
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="primary" disabled={busy}>
          {saveLabel}
        </Button>
        <Button variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
