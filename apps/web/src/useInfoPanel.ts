// Behaviour of the lightbox's Info panel that has nothing to do with what it shows: a history entry so the browser's
// Back button (and a phone's back gesture) closes the panel before it leaves the page.
import { useCallback, useEffect, useState } from "react";

import { createHistoryEntry, type HistoryEntry } from "./historyEntry";

/**
 * Open/closed state of the panel, backed by one history entry while it is open. `hide` goes back through history (so the
 * entry is not left behind); Back from the browser closes the panel by itself. `leave` pops the entry and resolves when
 * that has happened, for closing the lightbox or following a link without a stray entry in the history. The entry is
 * popped at most once, however many closes arrive before the browser has finished (see historyEntry.ts).
 */
export function useInfoHistory() {
  const [open, setOpen] = useState(false);
  const [e] = useState<HistoryEntry>(() => createHistoryEntry("cyInfo"));

  const show = useCallback(() => {
    e.push();
    setOpen(true);
  }, [e]);

  const hide = useCallback(() => {
    void e.pop(); // the popstate listener below closes the panel
    if (!e.has()) setOpen(false);
  }, [e]);

  const leave = useCallback(() => e.pop(), [e]);

  const toggle = useCallback(() => (open ? hide() : show()), [open, hide, show]);

  useEffect(() => {
    const on = () => {
      if (!e.has()) setOpen(false);
    };
    window.addEventListener("popstate", on);
    return () => window.removeEventListener("popstate", on);
  }, [e]);

  // The lightbox went away with the panel open (the photo was trashed...): take the entry with it.
  useEffect(() => () => void e.pop(), [e]);

  return { open, show, hide, leave, toggle };
}
