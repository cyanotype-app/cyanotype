import { useMemo, useState } from "react";
import { recoveryCodesFile } from "../twoFactorText";

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
    <section className="recovery" aria-label="Recovery codes">
      <h2>Your recovery codes</h2>
      <p>
        <strong>Save these now. They will not be shown again.</strong> Each one works once, if you lose your phone: enter
        it where the 6-digit code goes.
      </p>
      <ul className="recovery-codes">
        {codes.map((c) => (
          <li key={c}>
            <code>{c}</code>
          </li>
        ))}
      </ul>
      <div className="recovery-actions">
        <a className="btn btn-primary" href={url} download="cyanotype-recovery-codes.txt">
          Download
        </a>
        <button type="button" className="btn btn-quiet" onClick={() => void copy()}>
          Copy
        </button>
        <button type="button" className="btn btn-quiet" onClick={() => window.print()}>
          Print
        </button>
      </div>
      <div role="status" className="muted small">
        {note}
      </div>
      <label className="check">
        <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} /> I have saved these codes
        somewhere safe
      </label>
      <button type="button" className="btn btn-primary" disabled={!saved} onClick={onDone}>
        Done
      </button>
    </section>
  );
}
