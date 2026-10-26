import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { X } from "lucide-react";
import { useEffect, useRef, type ReactNode } from "react";
import { Button } from "../ui/Button";
import { Sheet } from "../ui/Sheet";
import { useDesktop } from "../ui/useMedia";

/** The width of the viewer's side panel on a desktop. */
const PANEL_W = 384;

interface Props {
  /** The id the panel's opening button points at (`aria-controls`). */
  id: string;
  /** The panel's name and heading ("Info", "Text"). */
  title: string;
  /** What assistive technology calls the side panel on a desktop ("Photo details"). */
  label: string;
  /** The close button's name on a desktop ("Close info"). */
  closeLabel: string;
  onClose: () => void;
  /** Whether it is shown (kept mounted while it animates away). */
  open?: boolean;
  children: ReactNode;
}

/**
 * The viewer's secondary panel (Info, Text): a bottom sheet on a phone, and on a desktop a column at the right that makes the
 * photo's area narrower instead of covering it. Both close with Escape and with their button, and neither keeps the photo from
 * being paged through behind it.
 */
export function ViewerPanel({ id, title, label, closeLabel, onClose, open = true, children }: Props) {
  const desktop = useDesktop();
  const reduce = useReducedMotion();
  const closeRef = useRef<HTMLButtonElement>(null);
  // Focus lands on the panel when it opens (the viewer hands it back to the button that opened it on close).
  useEffect(() => {
    if (desktop && open) closeRef.current?.focus();
  }, [desktop, open]);

  if (!desktop) {
    return (
      <Sheet open={open} onOpenChange={(o) => !o && onClose()} title={title} size="md" className="max-h-[78dvh]">
        <div id={id} className="pb-3">
          {children}
        </div>
      </Sheet>
    );
  }
  return (
    <AnimatePresence initial={false}>
      {open && (
        <motion.aside
          key={id}
          id={id}
          aria-label={label}
          className="shrink-0 overflow-hidden border-l border-line bg-card text-fg"
          initial={{ width: reduce ? PANEL_W : 0 }}
          animate={{ width: PANEL_W }}
          exit={{ width: reduce ? PANEL_W : 0 }}
          transition={reduce ? { duration: 0 } : { duration: 0.28, ease: [0.2, 0.8, 0.2, 1] }}
        >
          <div className="flex h-full flex-col" style={{ width: PANEL_W }}>
            <header className="flex shrink-0 items-center justify-between gap-2 px-5 pt-4 pb-2">
              <h2 className="text-[1.0625rem] font-semibold tracking-tight">{title}</h2>
              <Button ref={closeRef} variant="ghost" size="icon-sm" aria-label={closeLabel} onClick={onClose}>
                <X aria-hidden="true" />
              </Button>
            </header>
            <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 pb-6">{children}</div>
          </div>
        </motion.aside>
      )}
    </AnimatePresence>
  );
}
