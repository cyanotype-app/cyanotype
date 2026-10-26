import { useEffect, useState, type FormEvent } from "react";
import { useLocation } from "react-router-dom";
import { api, type ApiKeyScope, type CliRequest } from "../api";
import { codeFromHash, formatUserCode, userCodeComplete } from "../cliLoginText";
import { LinkButton } from "../components/AuthShell";
import { CliConsent, CliPage } from "../components/CliConsent";
import { Field, FormError, inputClass, Mono, Notice } from "../settings/kit";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";

/**
 * `/activate`: approve a command-line login from any device (the device-code flow, for SSH sessions and machines without a
 * browser). The person types the short code the terminal printed (or scans the QR code, which opens this page with
 * `#code=...`: only the fragment, which a browser never sends anywhere, and it is removed from the address at once); the code
 * goes to the server in a request body and is never part of a request address, so it ends up in no server log or referrer.
 */
export function Activate() {
  return <CliPage title="Sign in the command line">{(email) => <ActivateSteps email={email} />}</CliPage>;
}

type View =
  | { step: "enter" }
  | { step: "ask"; req: CliRequest; code: string }
  | { step: "done"; allowed: boolean; device: string };

function ActivateSteps({ email }: { email: string }) {
  const [view, setView] = useState<View>({ step: "enter" });
  // A scanned QR code opens /activate#code=...: the code is filled in, and the person still presses Continue and then Allow.
  const { hash } = useLocation();
  const [code, setCode] = useState(() => codeFromHash(hash));
  // Out of the address bar and the history once it is in the box.
  useEffect(() => {
    if (window.location.hash) window.history.replaceState(window.history.state, "", window.location.pathname + window.location.search);
  }, []);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function lookup(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setView({ step: "ask", req: await api.cliDeviceLookup(code), code });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not check that code.");
    } finally {
      setBusy(false);
    }
  }

  async function decide(allow: boolean, req: CliRequest, typed: string) {
    setBusy(true);
    setError(null);
    try {
      await (allow ? api.cliDeviceApprove(typed) : api.cliDeviceDeny(typed));
      setView({ step: "done", allowed: allow, device: req.device });
    } catch (err) {
      setError(err instanceof Error ? err.message : "That did not work.");
    } finally {
      setBusy(false);
    }
  }

  if (view.step === "done")
    return (
      <Notice tone="ok" role="status">
        {view.allowed
          ? `Allowed. The command line on ${view.device} is signing in; you can close this tab.`
          : `Denied. ${view.device} was not allowed. You can close this tab.`}
      </Notice>
    );
  if (view.step === "ask") {
    const { req, code: typed } = view;
    return (
      <div className="flex flex-col gap-4">
        <CliConsent
          device={req.device}
          scope={req.scope as ApiKeyScope}
          email={email}
          busy={busy}
          error={error}
          onAllow={() => void decide(true, req, typed)}
          onDeny={() => void decide(false, req, typed)}
        />
        <p className="text-center text-sm">
          <LinkButton onClick={() => setView({ step: "enter" })}>Enter a different code</LinkButton>
        </p>
      </div>
    );
  }
  return (
    <form onSubmit={lookup} className="flex flex-col gap-4">
      <p className="text-sm text-muted">
        Enter the code your terminal printed after you ran <Mono>cyanotype login</Mono>.
      </p>
      <Field label="Code">
        {(p) => (
          <input
            {...p}
            name="code"
            className={cn(inputClass, "h-14 text-center font-mono text-xl tracking-[0.18em] uppercase placeholder:tracking-[0.18em] md:text-xl")}
            value={code}
            onChange={(e) => setCode(formatUserCode(e.target.value))}
            autoFocus
            required
            autoComplete="off"
            autoCapitalize="characters"
            spellCheck={false}
            maxLength={9}
            placeholder="WDJB-MJHT"
          />
        )}
      </Field>
      <FormError>{error}</FormError>
      <Button type="submit" variant="primary" size="lg" disabled={busy || !userCodeComplete(code)}>
        {busy ? "Please wait…" : "Continue"}
      </Button>
    </form>
  );
}
