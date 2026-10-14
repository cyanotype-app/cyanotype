import { useEffect, useRef, type ReactNode } from "react";

interface Props {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  /** Styles the confirm button as destructive and focuses Cancel first, so Enter never destroys by accident. */
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/** A modal yes/no question. Escape cancels; Tab stays inside. */
export function ConfirmDialog({ title, children, confirmLabel, danger, busy, onConfirm, onCancel }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const buttons = ref.current?.querySelectorAll<HTMLButtonElement>("button");
    (danger ? buttons?.[0] : buttons?.[1])?.focus(); // Cancel first for destructive actions
    return () => prev?.focus?.();
  }, [danger]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      } else if (e.key === "Tab" && ref.current) {
        const f = ref.current.querySelectorAll<HTMLElement>("button");
        const first = f[0];
        const last = f[f.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last?.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first?.focus();
        }
      }
    }
    // Capture so Escape here never also clears a selection or closes the lightbox underneath.
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  return (
    <div className="modal-backdrop">
      <div className="modal card" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" ref={ref}>
        <h2 id="confirm-title">{title}</h2>
        <div className="modal-body">{children}</div>
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button
            type="button"
            className={`btn ${danger ? "btn-danger" : "btn-primary"}`}
            onClick={onConfirm}
            disabled={busy}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
