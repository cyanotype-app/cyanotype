import { useId, useState, type FormEvent } from "react";
import { Button } from "../ui/Button";
import { FormError, Modal, TextInput } from "./sheetForm";

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

/** A sheet with one text field (new album, rename a tag). Escape cancels. */
export function NameDialog({ title, label, submitLabel, initial = "", busy, error, onSubmit, onCancel }: Props) {
  const [value, setValue] = useState(initial);
  const form = useId();
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (value.trim()) onSubmit(value.trim());
  };
  return (
    <Modal
      onClose={onCancel}
      title={title}
      size="sm"
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={form} disabled={busy || !value.trim()}>
            {submitLabel}
          </Button>
        </>
      }
    >
      <form id={form} onSubmit={submit} className="flex flex-col gap-3">
        <label htmlFor={`${form}-input`} className="sr-only">
          {label}
        </label>
        <TextInput
          id={`${form}-input`}
          data-autofocus=""
          placeholder={label}
          value={value}
          maxLength={200}
          onChange={(e) => setValue(e.target.value)}
          disabled={busy}
          onFocus={(e) => e.currentTarget.select()}
        />
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
  );
}
