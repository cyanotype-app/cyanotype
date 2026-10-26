import { useMemo, useState } from "react";
import { recoveryCodesFile } from "../twoFactorText";
import { Button } from "../ui/Button";

/**
 * The recovery codes, shown once. The person must say they have saved them before the list goes away; the codes are never
 * readable again (the server keeps only a keyed hash). Download makes a text file in the browser, nothing is uploaded.
 */
export function RecoveryCodes({ codes, email, onDone }: { codes: string[]; email: string; onDone: () => void }) {
  const [saved, setSaved] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const url = useMemo(() => {
    const text = recoveryCodesFile(codes, email, window.location.origin);
    return URL.createObjectURL(new Blob([text], { type: "text/plain" }));
  }, [codes, email]);

  async function copy() {
    try {
      await navigator.clipboard.writeText(codes.join("\n"));
      setNote("Copied.");
    } catch {
      setNote("Could not copy automatically. Select the codes and copy them by hand.");
    }
  }

  return (
    <section aria-label="Recovery codes" className="flex flex-col gap-4 rounded-card bg-card p-4 ring-1 ring-line md:p-5">
      <div>
        <h3 className="text-[1.0625rem] font-semibold tracking-tight">Your recovery codes</h3>
        <p className="mt-1 text-[0.9375rem] text-muted">
          <strong className="font-semibold text-fg">Save these now. They will not be shown again.</strong> Each one works once, if you lose
          your phone: enter it where the 6-digit code goes.
        </p>
      </div>
      <ul className="grid grid-cols-1 gap-2 min-[420px]:grid-cols-2">
        {codes.map((c) => (
          <li key={c} className="rounded-xl bg-surface px-3 py-2.5 text-center">
            <code className="font-mono text-[0.9375rem] tracking-wide break-all">{c}</code>
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" asChild>
          <a href={url} download="cyanotype-recovery-codes.txt">
            Download
          </a>
        </Button>
        <Button onClick={() => void copy()}>Copy</Button>
        <Button onClick={() => window.print()}>Print</Button>
      </div>
      <div role="status" className="min-h-5 text-sm text-muted">
        {note}
      </div>
      <label className="flex items-start gap-3 text-[0.9375rem]">
        <input
          type="checkbox"
          className="mt-0.5 size-5 shrink-0 accent-accent"
          checked={saved}
          onChange={(e) => setSaved(e.target.checked)}
        />
        <span>I have saved these codes somewhere safe</span>
      </label>
      <div>
        <Button variant="primary" disabled={!saved} onClick={onDone}>
          Done
        </Button>
      </div>
    </section>
  );
}
