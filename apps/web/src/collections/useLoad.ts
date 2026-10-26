import { useEffect, useState } from "react";
import { ApiError } from "../api";
import { useAuth } from "../auth";

export type Loaded<T> = { status: "loading" } | { status: "ready"; data: T } | { status: "failed" };

/**
 * One section's data: loads once on mount, independently of its neighbours, so the page fills in section by section.
 * A failure is not shown as an error (a section that cannot load simply is not there); a 401 signs the user out.
 * `load` runs once per mount, so it need not be stable.
 */
export function useLoad<T>(load: () => Promise<T>, enabled = true): Loaded<T> {
  const { setAnonymous } = useAuth();
  const [state, setState] = useState<Loaded<T>>({ status: "loading" });
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    load().then(
      (data) => live && setState({ status: "ready", data }),
      (e: unknown) => {
        if (!live) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        setState({ status: "failed" });
      },
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, setAnonymous]);
  return state;
}
