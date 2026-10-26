import type { ReactNode } from "react";
import { X } from "lucide-react";
import { cn } from "../ui/cn";

/** A tag (or a person) as a pill: its name, usually a link, and an optional remove button. */
export function TagChip({
  children,
  onRemove,
  removeLabel,
  imported,
  title,
  disabled,
  className,
}: {
  children: ReactNode;
  onRemove?: () => void;
  removeLabel?: string;
  /** From the photo's own keywords: drawn dashed. */
  imported?: boolean;
  title?: string;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <span
      title={title}
      className={cn(
        "inline-flex min-h-8 max-w-full min-w-0 items-center gap-0.5 rounded-full border bg-surface pl-3 text-sm leading-tight",
        onRemove ? "pr-0.5" : "pr-3",
        imported ? "border-dashed border-line" : "border-transparent",
        className,
      )}
    >
      <span className="min-w-0 [overflow-wrap:anywhere] [&_a]:text-inherit [&_a]:no-underline [&_a:hover]:underline">{children}</span>
      {onRemove && (
        <button
          type="button"
          aria-label={removeLabel}
          disabled={disabled}
          onClick={onRemove}
          className="inline-flex size-7 shrink-0 items-center justify-center rounded-full text-muted hover:bg-danger/15 hover:text-danger disabled:opacity-45"
        >
          <X className="size-3.5" aria-hidden="true" />
        </button>
      )}
    </span>
  );
}
