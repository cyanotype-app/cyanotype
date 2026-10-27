import { useId, useState, type FormEvent } from "react";
import { Button } from "../ui/Button";
import { FormError, Modal, TextInput } from "./sheetForm";

/**
 * Rename, or give a name: a sheet with one field. `problem` says why a name cannot be kept (shown under the field; the sheet stays
 * open). People may clear their name (a blank is fine); a pet may not, which is `problem`'s business.
 */
export function RenameSheet({
  initial,
  title,
  problem,
  onSave,
  onCancel,
}: {
  initial: string;
  title: string;
  problem: (name: string) => string | null;
  onSave: (name: string) => void;
  onCancel: () => void;
}) {
  const id = useId();
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  function submit(e: FormEvent) {
    e.preventDefault();
    const p = problem(value);
    if (p) setError(p);
    else onSave(value);
  }
  return (
    <Modal
      onClose={onCancel}
      title={title}
      size="sm"
      footer={
        <>
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id}>
            Save
          </Button>
        </>
      }
    >
      <form id={id} className="flex flex-col gap-3" onSubmit={submit}>
        <label htmlFor={`${id}-name`} className="sr-only">
          Name
        </label>
        <TextInput
          id={`${id}-name`}
          data-autofocus=""
          placeholder="Name"
          value={value}
          maxLength={100}
          onChange={(e) => {
            setValue(e.target.value);
            setError(null);
          }}
          aria-invalid={error ? true : undefined}
        />
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
  );
}
