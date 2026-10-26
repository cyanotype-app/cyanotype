import type { ReactNode } from "react";
import { Button } from "../ui/Button";
import { Modal } from "./sheetForm";

/**
 * A yes/no question as a sheet. A destructive one (`danger`) puts focus on Cancel, so Enter never destroys by accident. Escape
 * cancels.
 */
export function ConfirmSheet({
  title,
  children,
  confirmLabel,
  danger,
  busy,
  onConfirm,
  onCancel,
}: {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Modal
      onClose={onCancel}
      title={title}
      size="sm"
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy} data-autofocus={danger ? "" : undefined}>
            Cancel
          </Button>
          <Button variant={danger ? "danger" : "primary"} onClick={onConfirm} disabled={busy} data-autofocus={danger ? undefined : ""}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-[0.9375rem] text-muted [overflow-wrap:anywhere]">{children}</div>
    </Modal>
  );
}
