import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { api } from "./api";

/** `null` until the server has said whether People is switched on (`FACES_ENABLED`, off by default); `false` outside a provider. */
const PeopleContext = createContext<boolean | null>(false);

/** Asks once, for the signed-in shell, whether People is on. A failed answer counts as off: nothing about People is shown. */
export function PeopleProvider({ children }: { children: ReactNode }) {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  useEffect(() => {
    let live = true;
    api
      .peopleStatus()
      .then((r) => live && setEnabled(r.enabled === true))
      .catch(() => live && setEnabled(false));
    return () => {
      live = false;
    };
  }, []);
  return <PeopleContext.Provider value={enabled}>{children}</PeopleContext.Provider>;
}

/** Is People on? `null` while it is not known yet. */
export const usePeopleEnabled = () => useContext(PeopleContext);
