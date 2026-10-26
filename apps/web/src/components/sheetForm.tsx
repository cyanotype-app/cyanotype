import { ChevronDown } from "lucide-react";
import {
  forwardRef,
  useEffect,
  useEffectEvent,
  type ComponentProps,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
} from "react";
import { cn } from "../ui/cn";
import { Sheet } from "../ui/Sheet";

/**
 * The pieces the albums, people, tags and sharing sheets are made of: `Modal` (a `Sheet` that is mounted while it is wanted),
 * fields, selects, check rows and an error line. They keep the look of the forms the same everywhere.
 */

/**
 * A sheet for a dialog that its page mounts and unmounts (`{naming && <NameDialog …/>}`). Escape closes it before the viewer or
 * the page behind hears it, focus goes to the first `data-autofocus` control (else the sheet picks), and back to where it was
 * when the sheet goes.
 */
export function Modal({ onClose, ...sheet }: Omit<ComponentProps<typeof Sheet>, "open" | "onOpenChange"> & { onClose: () => void }) {
  const close = useEffectEvent(onClose);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    // The sheet slides in from below: focusing a field now must not scroll the (overflow-hidden) dialog to bring it into view, and
    // nothing may leave it scrolled afterwards.
    const dialogs = document.querySelectorAll<HTMLDialogElement>("dialog[open]");
    const dialog = dialogs[dialogs.length - 1];
    const stay = () => {
      dialog?.scrollTo(0, 0);
    };
    dialog?.addEventListener("scroll", stay);
    dialog?.querySelector<HTMLElement>("[data-autofocus]")?.focus({ preventScroll: true });
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      close();
    }
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      dialog?.removeEventListener("scroll", stay);
      prev?.focus?.();
    };
  }, []);
  return <Sheet open onOpenChange={(o) => !o && onClose()} {...sheet} title={<span className="[overflow-wrap:anywhere]">{sheet.title}</span>} />;
}

export const inputClass =
  "block h-11 w-full rounded-card bg-surface px-3.5 text-base text-fg outline-none ring-1 ring-line transition-shadow " +
  "placeholder:text-muted focus-visible:ring-2 focus-visible:ring-focus disabled:opacity-55 md:text-[0.9375rem]";

export const TextInput = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(function TextInput(
  { className, ...props },
  ref,
) {
  return <input ref={ref} className={cn(inputClass, className)} {...props} />;
});

/** A native `<select>` (the phone's own picker is the best one there is), drawn like the other fields. */
export const SelectBox = forwardRef<HTMLSelectElement, SelectHTMLAttributes<HTMLSelectElement>>(function SelectBox(
  { className, children, ...props },
  ref,
) {
  return (
    <span className="relative block">
      <select ref={ref} className={cn(inputClass, "appearance-none pr-10", className)} {...props}>
        {children}
      </select>
      <ChevronDown className="pointer-events-none absolute top-1/2 right-3.5 size-4 -translate-y-1/2 text-muted" aria-hidden="true" />
    </span>
  );
});

/** A labelled field: the label above, an optional hint below. */
export function Field({ label, htmlFor, hint, children, className }: { label: ReactNode; htmlFor?: string; hint?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      <label htmlFor={htmlFor} className="text-[0.8125rem] font-medium text-muted">
        {label}
      </label>
      {children}
      {hint && <p className="text-[0.8125rem] text-muted">{hint}</p>}
    </div>
  );
}

/** A tick box with its words, the whole row tappable. */
export function CheckRow({
  checked,
  onChange,
  disabled,
  children,
  hint,
}: {
  checked: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
  children: ReactNode;
  hint?: ReactNode;
}) {
  return (
    <label className="flex min-h-11 cursor-pointer items-start gap-3 py-2.5 text-[0.9375rem] has-[:disabled]:cursor-default has-[:disabled]:opacity-55">
      <input
        type="checkbox"
        className="mt-0.5 size-5 shrink-0 accent-accent"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="min-w-0">
        {children}
        {hint && <span className="block text-[0.8125rem] text-muted">{hint}</span>}
      </span>
    </label>
  );
}

/** An error the user should hear about now. */
export function FormError({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <p role="alert" className={cn("text-sm text-danger [overflow-wrap:anywhere]", className)}>
      {children}
    </p>
  );
}

/** A quiet line of explanation. */
export function Note({ children, className, role }: { children: ReactNode; className?: string; role?: "status" | "note" }) {
  return (
    <p role={role} className={cn("text-[0.8125rem] text-muted [overflow-wrap:anywhere]", className)}>
      {children}
    </p>
  );
}
