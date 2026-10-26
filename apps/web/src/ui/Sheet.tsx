import { AnimatePresence, motion, useDragControls, useReducedMotion, type PanInfo } from "motion/react";
import { X } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "./cn";
import { useDesktop } from "./useMedia";

interface SheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  /** Shown under the title, and read out with it. */
  description?: ReactNode;
  /** Hide the title visually (it is still the dialog's name). */
  hideTitle?: boolean;
  /** Buttons pinned under the content (a sheet's "Save", "Cancel"). */
  footer?: ReactNode;
  /** `sm` for questions and short forms, `lg` for pickers and lists. Desktop only: a phone sheet is full width. */
  size?: "sm" | "md" | "lg";
  /** `alertdialog` for a question that wants an answer (ConfirmDialog). */
  role?: "dialog" | "alertdialog";
  className?: string;
  children?: ReactNode;
}

const DISMISS_DISTANCE = 96;
const DISMISS_VELOCITY = 600;

/**
 * A modal panel: a bottom sheet on a phone (drag its handle or header down to close), a centred dialog on a desktop.
 * Built on `<dialog>` so focus stays inside, Escape closes and the page behind is inert, with no style element at
 * runtime (the gateway's CSP allows none). The page behind does not scroll while one is open (theme.css).
 */
export function Sheet({ open, onOpenChange, title, description, hideTitle, footer, size = "md", role, className, children }: SheetProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const [mounted, setMounted] = useState(open);
  const desktop = useDesktop();
  const reduce = useReducedMotion();
  const drag = useDragControls();
  const titleId = useId();
  const descId = useId();

  if (open && !mounted) setMounted(true);

  useLayoutEffect(() => {
    const d = ref.current;
    if (mounted && d && !d.open) d.showModal?.();
  }, [mounted]);

  // Escape: animate out like any other close instead of the dialog vanishing.
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    const onCancel = (e: Event) => {
      e.preventDefault();
      onOpenChange(false);
    };
    d.addEventListener("cancel", onCancel);
    return () => d.removeEventListener("cancel", onCancel);
  }, [mounted, onOpenChange]);

  if (!mounted) return null;

  const onDragEnd = (_: unknown, info: PanInfo) => {
    if (info.offset.y > DISMISS_DISTANCE || info.velocity.y > DISMISS_VELOCITY) onOpenChange(false);
  };
  const panel = desktop
    ? { initial: { opacity: 0, scale: 0.97, y: 8 }, animate: { opacity: 1, scale: 1, y: 0 }, exit: { opacity: 0, scale: 0.98, y: 4 } }
    : { initial: { y: "100%" }, animate: { y: 0 }, exit: { y: "100%" } };

  return (
    <dialog
      ref={ref}
      role={role}
      aria-labelledby={titleId}
      aria-describedby={description ? descId : undefined}
      className="fixed inset-0 m-0 h-dvh max-h-none w-full max-w-none overflow-hidden bg-transparent p-0 text-fg backdrop:bg-transparent"
    >
      <AnimatePresence
        onExitComplete={() => {
          ref.current?.close();
          setMounted(false);
        }}
      >
        {open && (
          <div key="sheet" className="flex h-full items-end justify-center md:items-center md:p-6">
            <motion.div
              className="absolute inset-0 bg-scrim"
              aria-hidden="true"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: reduce ? 0 : 0.2 }}
              onClick={() => onOpenChange(false)}
            />
            <motion.div
              role="document"
              className={cn(
                "relative flex max-h-[92dvh] w-full flex-col bg-card shadow-float",
                "rounded-t-sheet pb-[env(safe-area-inset-bottom,0px)] md:rounded-sheet md:pb-0",
                size === "sm" && "md:max-w-sm",
                size === "md" && "md:max-w-lg",
                size === "lg" && "md:max-w-2xl",
                className,
              )}
              {...panel}
              transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 520, damping: 44, mass: 0.9 }}
              drag={desktop ? false : "y"}
              dragControls={drag}
              dragListener={false}
              dragConstraints={{ top: 0, bottom: 0 }}
              dragElastic={{ top: 0.04, bottom: 0.7 }}
              onDragEnd={onDragEnd}
            >
              <header
                className="shrink-0 touch-none px-5 pt-2 pb-3 md:touch-auto md:pt-5"
                onPointerDown={(e) => !desktop && drag.start(e)}
              >
                <div className="mx-auto mb-3 h-1.5 w-10 rounded-full bg-line md:hidden" aria-hidden="true" />
                <div className="flex items-start gap-3">
                  <div className={cn("min-w-0 flex-1", hideTitle && "sr-only")}>
                    <h2 id={titleId} className="text-[1.0625rem] font-semibold tracking-tight text-balance">
                      {title}
                    </h2>
                    {description && (
                      <p id={descId} className="mt-0.5 text-sm text-muted">
                        {description}
                      </p>
                    )}
                  </div>
                  <button
                    type="button"
                    aria-label="Close"
                    onClick={() => onOpenChange(false)}
                    className="-mt-1 -mr-2 ml-auto hidden size-9 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-fg md:inline-flex"
                  >
                    <X className="size-[1.15rem]" />
                  </button>
                </div>
              </header>
              <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 pb-5">{children}</div>
              {footer && (
                <footer className="flex shrink-0 flex-col-reverse gap-2 border-t border-line px-5 py-3 md:flex-row md:justify-end">
                  {footer}
                </footer>
              )}
            </motion.div>
          </div>
        )}
      </AnimatePresence>
    </dialog>
  );
}
