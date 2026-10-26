import { useState } from "react";
import type { Target } from "../api";
import { Button } from "../ui/Button";
import { TagDialog } from "./TagDialog";

interface Props {
  target: Target;
  count: number;
  disabled?: boolean;
  /** The server applied the change; the page refreshes and shows `message`. */
  onDone: (message: string) => void;
}

/** "Tag" for the selection bar: the add/remove sheet for the selected photos. */
export function TagTools({ target, count, disabled, onDone }: Props) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="quiet" size="sm" disabled={disabled} onClick={() => setOpen(true)}>
        Tag
      </Button>
      {open && (
        <TagDialog
          target={target}
          count={count}
          onClose={() => setOpen(false)}
          onDone={(r) => {
            setOpen(false);
            onDone(r.message);
          }}
        />
      )}
    </>
  );
}
