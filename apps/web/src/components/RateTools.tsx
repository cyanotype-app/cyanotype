import { useState, type ReactNode } from "react";
import { api, type Target } from "../api";
import { ratedMessage, starsLabel } from "../ratingText";
import { Button } from "../ui/Button";
import { Sheet } from "../ui/Sheet";
import { StarIcon } from "./Stars";

interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Which photos: the selection (ids, or a whole view). */
  target: Target;
  count: number;
  /** The server rated them (`changed` is how many actually changed); the page refreshes and tells the owner. */
  onDone: (rating: number, changed: number, message: string) => void;
  /** The request failed (`error` is what went wrong, for the page's own handling of a signed-out session). */
  onError: (error: unknown) => void;
}

/** The stars to give the selected photos (0 clears), as a sheet. */
export function RateDialog({ open, onOpenChange, target, count, onDone, onError }: DialogProps) {
  const [busy, setBusy] = useState(false);

  async function rate(rating: number) {
    setBusy(true);
    try {
      const r = await api.rateAssets(target, rating);
      onOpenChange(false);
      onDone(rating, r.count, ratedMessage(count, rating));
    } catch (e) {
      onOpenChange(false);
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      open={open}
      onOpenChange={onOpenChange}
      size="sm"
      title="Rate photos"
      description={`${count.toLocaleString("en-US")} ${count === 1 ? "photo" : "photos"}`}
      footer={
        <Button variant="secondary" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
      }
    >
      <div className="flex flex-col gap-1">
        {[5, 4, 3, 2, 1, 0].map((n) => (
          <button
            key={n}
            type="button"
            className="flex min-h-12 items-center gap-3 rounded-xl px-3 text-left text-[0.9375rem] hover:bg-surface disabled:opacity-50"
            disabled={busy}
            onClick={() => void rate(n)}
            autoFocus={n === 5}
          >
            <span className="flex w-28 shrink-0 gap-0.5 text-muted" aria-hidden="true">
              {Array.from({ length: 5 }, (_, i) => (
                <span key={i} className={i < n ? "size-5 text-(--star)" : "size-5"}>
                  <StarIcon filled={i < n} />
                </span>
              ))}
            </span>
            <span>{n === 0 ? "Clear rating" : starsLabel(n)}</span>
          </button>
        ))}
      </div>
    </Sheet>
  );
}

interface Props extends Omit<DialogProps, "open" | "onOpenChange"> {
  disabled?: boolean;
  /** Your own button instead of the plain "Rate": it is given the call that opens the sheet. */
  trigger?: (open: () => void) => ReactNode;
}

/** "Rate" for a page's selection bar: a button, and the sheet of stars it opens. */
export function RateTools({ disabled, trigger, ...rest }: Props) {
  const [open, setOpen] = useState(false);
  return (
    <>
      {trigger ? (
        trigger(() => setOpen(true))
      ) : (
        <Button variant="quiet" size="sm" disabled={disabled} onClick={() => setOpen(true)}>
          Rate
        </Button>
      )}
      <RateDialog open={open} onOpenChange={setOpen} {...rest} />
    </>
  );
}
