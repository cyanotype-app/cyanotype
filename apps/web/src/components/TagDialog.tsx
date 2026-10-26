import { useEffect, useId, useState, type FormEvent } from "react";
import { api, ApiError, type Target } from "../api";
import { tagPhotos, taggedMessage } from "../tagText";
import { Button } from "../ui/Button";
import { Sheet } from "../ui/Sheet";
import { TagInput } from "./TagInput";

interface Props {
  /** Which photos: the selection (ids, or a whole view). */
  target: Target;
  count: number;
  /** Offer the "remove" field too (the selection bar); the single-photo "Add tag" in the viewer does not. */
  allowRemove?: boolean;
  onClose: () => void;
  /** The server applied it: `added` and `removed` are the links that changed, `message` is for the toast. */
  onDone: (r: { added: number; removed: number; message: string }) => void;
  /** Names to start the add field with (the tag page's own tag, for "also tag"). */
  initialAdd?: readonly string[];
}

/** Add tags to, and take tags off, the selected photos, in a sheet. Escape cancels; Apply is Enter in the sheet. */
export function TagDialog({ target, count, allowRemove = true, onClose, onDone, initialAdd = [] }: Props) {
  const [add, setAdd] = useState<readonly string[]>(initialAdd);
  const [remove, setRemove] = useState<readonly string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const formId = useId();

  // Focus goes back to what opened the sheet once it is gone (read while rendering: the sheet moves focus into itself as it mounts).
  const [opener] = useState(() => document.activeElement as HTMLElement | null);
  useEffect(() => () => opener?.focus?.(), [opener]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (add.length === 0 && remove.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const r = await api.applyTags(target, { add, remove });
      onDone({ ...r, message: taggedMessage(count, add, remove) });
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) throw err;
      setError(err instanceof Error ? err.message : "Could not change the tags.");
      setBusy(false);
    }
  }

  return (
    <Sheet
      open
      onOpenChange={(o) => !o && !busy && onClose()}
      title={`Tag ${tagPhotos(count)}`}
      size="sm"
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="primary" disabled={busy || (add.length === 0 && remove.length === 0)}>
            Apply
          </Button>
        </>
      }
    >
      <form id={formId} className="flex flex-col gap-4 pb-2" onSubmit={(e) => void submit(e)}>
        <TagInput value={add} onChange={setAdd} label="Add tags" placeholder="beach, trip/2024" disabled={busy} autoFocus />
        {allowRemove && <TagInput value={remove} onChange={setRemove} label="Remove tags" placeholder="Tags to take off" existingOnly disabled={busy} />}
        {error && (
          <p role="alert" className="text-sm text-danger">
            {error}
          </p>
        )}
      </form>
    </Sheet>
  );
}
