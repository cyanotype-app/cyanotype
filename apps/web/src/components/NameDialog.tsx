import { useEffect, useRef, useState, type FormEvent } from "react";

interface Props {
  title: string;
  label: string;
  submitLabel: string;
  initial?: string;
  busy?: boolean;
  error?: string | null;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}

/** A modal with one text field (new album). Escape cancels. */
export function NameDialog({ title, label, submitLabel, initial = "", busy, error, onSubmit, onCancel }: Props) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLFormElement>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector("input")?.focus();
    return () => prev?.focus?.();
  }, []);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (value.trim()) onSubmit(value.trim());
  };

  return (
    <div className="modal-backdrop">
      <form className="modal card" role="dialog" aria-modal="true" aria-labelledby="name-title" ref={ref} onSubmit={submit}>
        <h2 id="name-title">{title}</h2>
        <label htmlFor="name-input" className="sr-only">
          {label}
        </label>
        <input
          id="name-input"
          className="text-input"
          placeholder={label}
          value={value}
          maxLength={200}
          onChange={(e) => setValue(e.target.value)}
          disabled={busy}
        />
        {error && <p role="alert" className="form-error">{error}</p>}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || !value.trim()}>
            {submitLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
