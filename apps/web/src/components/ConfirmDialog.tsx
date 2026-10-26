import { useEffect, useRef, type ReactNode } from "react";
import { Button } from "../ui/Button";
import { Sheet } from "../ui/Sheet";

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

/** A modal yes/no question, a small sheet. Escape cancels; focus stays inside and returns to where it was. */
export function ConfirmDialog({ title, children, confirmLabel, danger, busy, onConfirm, onCancel }: Props) {
  const cancel = useRef<HTMLButtonElement>(null);
  const confirm = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    // Cancel first for destructive actions (the sheet has already moved focus to its first control by now).
    (danger ? cancel : confirm).current?.focus();
    return () => prev?.focus?.();
  }, [danger]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    }
    // Capture so Escape here never also clears a selection or closes the lightbox underneath.
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  return (
    <Sheet
      open
      onOpenChange={(o) => !o && onCancel()}
      title={title}
      size="sm"
      role="alertdialog"
      footer={
        <>
          <Button ref={cancel} variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button ref={confirm} variant={danger ? "danger" : "primary"} onClick={onConfirm} disabled={busy}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="text-[0.9375rem] leading-relaxed text-muted [&_strong]:font-semibold [&_strong]:text-fg">{children}</div>
    </Sheet>
  );
}
