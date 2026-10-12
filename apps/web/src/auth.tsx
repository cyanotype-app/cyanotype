import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { api, ApiError, type Me } from "./api";

type State = { status: "loading" } | { status: "anonymous" } | { status: "authenticated"; user: Me };

interface AuthValue {
  state: State;
  /** Re-reads GET /auth/me (after login/register). */
  refresh: () => Promise<void>;
  signOut: () => Promise<void>;
  setAnonymous: () => void;
}

const AuthContext = createContext<AuthValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<State>({ status: "loading" });

  const refresh = useCallback(async () => {
    try {
      setState({ status: "authenticated", user: await api.me() });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setState({ status: "anonymous" });
      else throw e;
    }
  }, []);

  useEffect(() => {
    // A failing /auth/me other than 401 (server down) is treated as signed out; the login form then reports the problem.
    refresh().catch(() => setState({ status: "anonymous" }));
  }, [refresh]);

  const signOut = useCallback(async () => {
    try {
      await api.logout();
    } finally {
      setState({ status: "anonymous" });
    }
  }, []);

  const setAnonymous = useCallback(() => setState({ status: "anonymous" }), []);

  const value = useMemo(() => ({ state, refresh, signOut, setAnonymous }), [state, refresh, signOut, setAnonymous]);
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthValue {
  const v = useContext(AuthContext);
  if (!v) throw new Error("useAuth outside AuthProvider");
  return v;
}
