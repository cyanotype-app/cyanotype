// The small parts the settings, admin and sign-in pages share: grouped sections, rows, fields, notices. Plain Tailwind
// on the design tokens, so they follow the light and dark themes without a rule of their own.
import { AlertCircle, CheckCircle2, Info } from "lucide-react";
import { useId, type ComponentPropsWithoutRef, type ReactNode } from "react";
import { cn } from "../ui/cn";

/** A section of a settings page: a 17px title, a quiet line under it, then its content. */
export function Section({
  title,
  description,
  actions,
  children,
  className,
  headingLevel = 2,
  id,
  label,
}: {
  title?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
  headingLevel?: 2 | 3;
  id?: string;
  /** Names the region when it has no visible title. */
  label?: string;
}) {
  const auto = useId();
  const hid = id ?? auto;
  const H = headingLevel === 2 ? "h2" : "h3";
  return (
    <section aria-labelledby={title ? hid : undefined} aria-label={title ? undefined : label} className={cn("mt-8 first:mt-0", className)}>
      {(title || actions) && (
        <div className="mb-2 flex items-end gap-3">
          <div className="min-w-0 flex-1">
            {title && (
              <H id={hid} className="text-[1.0625rem] font-semibold tracking-tight">
                {title}
              </H>
            )}
            {description && <p className="mt-0.5 text-sm text-muted">{description}</p>}
          </div>
          {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
        </div>
      )}
      {children}
    </section>
  );
}

/** An iOS-style group: rows on one rounded well, hairlines between them. */
export function Group({
  children,
  className,
  as: Tag = "div",
  ...rest
}: { children: ReactNode; className?: string; as?: "div" | "ul" } & Omit<ComponentPropsWithoutRef<"div">, "children" | "className">) {
  return (
    <Tag className={cn("divide-y divide-line overflow-hidden rounded-card bg-card ring-1 ring-line", className)} {...(rest as object)}>
      {children}
    </Tag>
  );
}

/** One row of a group: a label (and detail) on the left, a value or control on the right; it stacks on a narrow phone. */
export function Row({
  label,
  detail,
  children,
  className,
  as: Tag = "div",
}: {
  label?: ReactNode;
  detail?: ReactNode;
  children?: ReactNode;
  className?: string;
  as?: "div" | "li";
}) {
  return (
    <Tag className={cn("flex min-h-12 flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5", className)}>
      {(label || detail) && (
        <div className="min-w-0 flex-1 basis-40">
          {label && <div className="text-[0.9375rem] [overflow-wrap:anywhere]">{label}</div>}
          {detail && <div className="text-[0.8125rem] text-muted [overflow-wrap:anywhere]">{detail}</div>}
        </div>
      )}
      {children !== undefined && children !== null && (
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-2 text-[0.9375rem]">{children}</div>
      )}
    </Tag>
  );
}

/** A group's own padded block (a form, a note) rather than a label/value row. */
export function Block({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("px-4 py-3.5", className)}>{children}</div>;
}

export const inputClass =
  "h-11 w-full min-w-0 rounded-xl bg-surface px-3.5 text-base text-fg ring-1 ring-line placeholder:text-muted/70 " +
  "focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-focus disabled:opacity-50 md:text-[0.9375rem]";

/** A labelled field with an optional hint and inline error. Pass the control the ids through the render function. */
export function Field({
  label,
  hint,
  error,
  children,
  className,
}: {
  label: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  children: (p: { id: string; "aria-describedby"?: string; "aria-invalid"?: boolean }) => ReactNode;
  className?: string;
}) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errId = `${id}-err`;
  const describedBy = [hint ? hintId : null, error ? errId : null].filter(Boolean).join(" ") || undefined;
  return (
    <div className={cn("flex min-w-0 flex-col gap-1.5", className)}>
      <label htmlFor={id} className="text-sm font-medium">
        {label}
      </label>
      {children({ id, "aria-describedby": describedBy, "aria-invalid": error ? true : undefined })}
      {hint && (
        <p id={hintId} className="text-[0.8125rem] text-muted">
          {hint}
        </p>
      )}
      {error && (
        <p id={errId} role="alert" className="text-[0.8125rem] text-danger">
          {error}
        </p>
      )}
    </div>
  );
}

/** A select with its own chevron (the native arrow is removed so it looks the same everywhere). */
export function SelectBox({ className, children, ...props }: ComponentPropsWithoutRef<"select">) {
  return (
    <div className="relative min-w-0">
      <select className={cn(inputClass, "appearance-none pr-9", className)} {...props}>
        {children}
      </select>
      <svg viewBox="0 0 20 20" aria-hidden="true" className="pointer-events-none absolute top-1/2 right-3 size-4 -translate-y-1/2 text-muted">
        <path d="m5 8 5 5 5-5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </div>
  );
}

/**
 * The live error line of a form. It is always in the page (empty when there is nothing to say) so a screen reader
 * announces the message when it appears.
 */
export function FormError({ children, className }: { children?: ReactNode; className?: string }) {
  return (
    <div role="alert" className={cn(children ? "flex items-start gap-2 text-sm text-danger" : "sr-only", className)}>
      {children && <AlertCircle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />}
      <span className="min-w-0 [overflow-wrap:anywhere]">{children}</span>
    </div>
  );
}

/** A status line (an inline confirmation): announced politely, quiet. */
export function StatusNote({ children, className }: { children?: ReactNode; className?: string }) {
  if (!children) return null;
  return (
    <p role="status" className={cn("flex items-start gap-2 text-sm text-muted", className)}>
      <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden="true" />
      <span className="min-w-0 [overflow-wrap:anywhere]">{children}</span>
    </p>
  );
}

/** A boxed notice: information by default, `warn` when something needs doing. */
export function Notice({
  tone = "info",
  children,
  role,
  className,
}: {
  tone?: "info" | "warn" | "ok";
  children: ReactNode;
  role?: "alert" | "status";
  className?: string;
}) {
  const Icon = tone === "ok" ? CheckCircle2 : tone === "warn" ? AlertCircle : Info;
  return (
    <div role={role} className={cn("flex items-start gap-3 rounded-card bg-surface px-4 py-3 text-sm", className)}>
      <Icon className={cn("mt-0.5 size-[1.1rem] shrink-0", tone === "warn" ? "text-danger" : "text-accent")} aria-hidden="true" />
      <div className="min-w-0 flex-1 [overflow-wrap:anywhere]">{children}</div>
    </div>
  );
}

/** "Loading…" for a page that is waiting for its data. */
export function Loading({ children = "Loading…" }: { children?: ReactNode }) {
  return (
    <p className="py-6 text-sm text-muted" role="status">
      {children}
    </p>
  );
}

/** Code-ish text (a command, a prefix) that wraps instead of overflowing. */
export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return <code className={cn("rounded-md bg-surface px-1.5 py-0.5 font-mono text-[0.8125rem] break-all", className)}>{children}</code>;
}

/** A secret shown once (a key, a link), with its buttons under it. */
export function SecretBox({ label, title, children, actions }: { label: string; title: ReactNode; children: ReactNode; actions: ReactNode }) {
  return (
    <section aria-label={label} className="rounded-card bg-surface p-4">
      <p className="text-[0.9375rem] font-semibold">{title}</p>
      <code className="mt-2 block rounded-xl bg-bg p-3 font-mono text-[0.8125rem] break-all ring-1 ring-line">{children}</code>
      <div className="mt-3 flex flex-wrap gap-2">{actions}</div>
    </section>
  );
}

/** An empty or failed page: a short title, a line of help, an optional action. */
export function Empty({ title, children, action, role }: { title?: ReactNode; children?: ReactNode; action?: ReactNode; role?: "alert" | "status" }) {
  return (
    <div role={role} className="mx-auto flex max-w-sm flex-col items-center gap-3 px-6 py-16 text-center">
      {title && <h2 className="text-[1.0625rem] font-semibold tracking-tight">{title}</h2>}
      {children && <div className="text-[0.9375rem] text-muted">{children}</div>}
      {action}
    </div>
  );
}

/** A titled card (a block of the health page): a heading, then free content. */
export function Card({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section aria-labelledby={id} className="rounded-card bg-card p-4 ring-1 ring-line md:p-5">
      <h2 id={id} className="text-[1.0625rem] font-semibold tracking-tight">
        {title}
      </h2>
      <div className="mt-2 flex flex-col gap-3 text-sm">{children}</div>
    </section>
  );
}
