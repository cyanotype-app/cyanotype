import { useEffect, useState } from "react";
import { useLocation } from "react-router-dom";
import { api, ApiError, type ApiKeyScope, type CliRequest } from "../api";
import { BAD_REDIRECT, authorizeParams, isLoopbackRedirect, nav } from "../cliLoginText";
import { CliConsent, CliPage } from "../components/CliConsent";

/**
 * `/cli/authorize`: where `cyanotype login` sends the browser. The command line listens on a port of this computer and
 * passed that address, a `state` and a PKCE challenge; pressing Allow makes the server issue a single-use code and the page
 * navigates to the listener with it (a navigation, which the Content-Security-Policy's form-action does not govern). The
 * server re-checks everything the address says, including that it is a loopback address and nothing else.
 */
export function CliAuthorize() {
  const search = useLocation().search;
  return (
    <CliPage title="Sign in the command line">{(email) => <Authorize search={search} email={email} />}</CliPage>
  );
}

type View =
  | { step: "checking" }
  | { step: "invalid"; message: string }
  | { step: "ask"; req: CliRequest }
  | { step: "done"; allowed: boolean };

function Authorize({ search, email }: { search: string; email: string }) {
  const params = authorizeParams(search);
  const [view, setView] = useState<View>(
    params ? { step: "checking" } : { step: "invalid", message: "This is not a login link from the cyanotype command line." },
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!params) return;
    let live = true;
    api
      .cliAuthorizeCheck(params)
      .then((req) => live && setView({ step: "ask", req }))
      .catch((e: unknown) => {
        if (live) setView({ step: "invalid", message: e instanceof Error ? e.message : "Could not read the login request." });
      });
    return () => {
      live = false;
    };
    // The request is fixed by the address; reading it again for the same address would only repeat the check.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search]);

  async function decide(allow: boolean) {
    if (!params) return;
    setBusy(true);
    setError(null);
    try {
      const { redirect } = await (allow ? api.cliAuthorizeApprove(params) : api.cliAuthorizeDeny(params));
      if (!isLoopbackRedirect(redirect)) {
        setError(BAD_REDIRECT);
        setBusy(false);
        return;
      }
      setView({ step: "done", allowed: allow });
      nav.to(redirect);
    } catch (e) {
      setError(e instanceof ApiError || e instanceof Error ? e.message : "That did not work.");
      setBusy(false);
    }
  }

  if (view.step === "checking")
    return (
      <p className="muted" role="status">
        Checking the request…
      </p>
    );
  if (view.step === "invalid")
    return (
      <div role="alert">
        <p>{view.message}</p>
        <p className="muted small">Run <code>cyanotype login</code> again to get a new link.</p>
      </div>
    );
  if (view.step === "done")
    return (
      <p role="status">
        {view.allowed ? "Allowed. You can close this tab and go back to your terminal." : "Denied. Nothing was allowed. You can close this tab."}
      </p>
    );
  return (
    <CliConsent
      device={view.req.device}
      scope={view.req.scope as ApiKeyScope}
      email={email}
      busy={busy}
      error={error}
      onAllow={() => void decide(true)}
      onDeny={() => void decide(false)}
    />
  );
}
