import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { api, type PetsSummary } from "./api";

/** What the shell knows about Pets: whether it is on (`PETS_ENABLED`, on by default) and, if so, the header counts. */
interface PetsState {
  enabled: boolean;
  summary: PetsSummary | null;
}

/** `null` until the server has said whether Pets is on; off outside a provider. */
const PetsContext = createContext<PetsState | null>({ enabled: false, summary: null });

/**
 * Asks once, for the signed-in shell, whether Pets is on and how many photos have one. A failed status counts as off: nothing about
 * Pets is shown. A failed summary leaves Pets on without entry points (they need a count); the Pets page reads its own.
 */
export function PetsProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<PetsState | null>(null);
  useEffect(() => {
    let live = true;
    const off: PetsState = { enabled: false, summary: null };
    api
      .petsStatus()
      .then(async (r): Promise<PetsState> => (r.enabled === true ? { enabled: true, summary: await api.pets().catch(() => null) } : off))
      .catch(() => off)
      .then((s) => live && setState(s));
    return () => {
      live = false;
    };
  }, []);
  return <PetsContext.Provider value={state}>{children}</PetsContext.Provider>;
}

/** Is Pets on? `null` while it is not known yet. */
export function usePetsEnabled(): boolean | null {
  const s = useContext(PetsContext);
  return s === null ? null : s.enabled;
}

/** The header counts the shell read, or null (off, unknown, or not readable). */
export const usePetsSummary = () => useContext(PetsContext)?.summary ?? null;

/** Show Pets entry points: it is on and at least one photo has a dog or a cat. */
export function useHasPets(): boolean {
  const s = useContext(PetsContext);
  return s !== null && s.enabled && (s.summary?.any ?? 0) > 0;
}
