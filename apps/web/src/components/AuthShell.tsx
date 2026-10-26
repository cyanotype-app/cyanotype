import type { ReactNode } from "react";
import { cn } from "../ui/cn";
import { Logo } from "./Logo";

/**
 * The frame of every signed-out page (sign in, register, password reset, approving a command-line login): a calm card in the
 * middle of a plain page, the logo above the title. No shell around it.
 */
export function AuthShell({ title, children, footer, wide }: { title: ReactNode; children: ReactNode; footer?: ReactNode; wide?: boolean }) {
  return (
    <main className="flex min-h-dvh items-center justify-center bg-bg px-4 py-10 text-fg">
      <div className={cn("w-full", wide ? "max-w-md" : "max-w-sm")}>
        <div className="flex flex-col items-center gap-3 pb-6 text-center">
          <Logo size={44} />
          <h1 className="text-[1.5rem] leading-[1.2] font-bold tracking-[-0.02em]">{title}</h1>
        </div>
        <div className="rounded-sheet bg-card p-5 ring-1 ring-line [overflow-wrap:anywhere] sm:p-7">{children}</div>
        {footer && <p className="mt-5 text-center text-sm text-muted [&_a]:text-accent">{footer}</p>}
      </div>
    </main>
  );
}

/** A button that reads as a link (switching between two ways of doing the same thing). */
export function LinkButton({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button type="button" className="rounded text-accent underline-offset-2 hover:underline" {...props}>
      {children}
    </button>
  );
}
