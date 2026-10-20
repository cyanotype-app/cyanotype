import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, ApiError, type Target } from "../api";
import { tagPhotos, taggedMessage } from "../tagText";
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

/** Add tags to, and take tags off, the selected photos. Escape cancels; Apply is Enter in the dialog. */
export function TagDialog({ target, count, allowRemove = true, onClose, onDone, initialAdd = [] }: Props) {
  const [add, setAdd] = useState<readonly string[]>(initialAdd);
  const [remove, setRemove] = useState<readonly string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLFormElement>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("input")?.focus();
    return () => prev?.focus?.();
  }, []);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    }
    // The suggestions list handles its own Escape first (it stops the event before it gets here).
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

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
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <form className="modal card tag-dialog" role="dialog" aria-modal="true" aria-labelledby="tag-dialog-title" ref={ref} onSubmit={(e) => void submit(e)}>
        <h2 id="tag-dialog-title">Tag {tagPhotos(count)}</h2>
        <TagInput value={add} onChange={setAdd} label="Add tags" placeholder="beach, trip/2024" disabled={busy} />
        {allowRemove && <TagInput value={remove} onChange={setRemove} label="Remove tags" placeholder="Tags to take off" existingOnly disabled={busy} />}
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || (add.length === 0 && remove.length === 0)}>
            Apply
          </button>
        </div>
      </form>
    </div>
  );
}
