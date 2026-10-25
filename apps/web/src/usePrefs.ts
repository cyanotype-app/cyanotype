import { useCallback, useEffect, useState } from "react";
import { api, type Prefs } from "./api";

const DEFAULTS: Prefs = { memories_enabled: true, library_sort: "taken", stacks_collapsed: true };

/**
 * The per-user switches (the On this day strip, the library's order), read once from the server. `prefs` is null while
 * they load (and when `enabled` is false); a failed read falls back to the defaults rather than blocking the page.
 * `update` shows a change at once and keeps it only if the server took it.
 */
export function usePrefs(enabled = true) {
  const [prefs, setPrefs] = useState<Prefs | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    api
      .prefs()
      .then((p) => live && setPrefs({ ...DEFAULTS, ...p }))
      .catch(() => live && setPrefs(DEFAULTS));
    return () => {
      live = false;
    };
  }, [enabled]);
  const update = useCallback(async (patch: Partial<Prefs>) => {
    let before: Prefs | null = null;
    setPrefs((p) => {
      before = p;
      return { ...(p ?? DEFAULTS), ...patch };
    });
    try {
      setPrefs({ ...DEFAULTS, ...(await api.setPrefs(patch)) });
    } catch {
      setPrefs(before);
    }
  }, []);
  return { prefs, update };
}
