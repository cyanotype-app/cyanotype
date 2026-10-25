import { useState } from "react";
import { MAX_ZIP_IDS, isScope, zipApi, zipBody, type Target } from "../api";
import { submitZip, zipStartedMessage } from "../zipDownload";

interface Props {
  /** Which photos: the selection (ids, or a whole view), or `{ scope: { album }, except: [] }` for a whole album. */
  target: Target;
  /** Names the file (the album's title); without it the server uses "cyanotype-photos.zip". */
  name?: string;
  disabled?: boolean;
  className?: string;
  /** The archive is on its way (the browser shows the progress); the page tells the owner. */
  onStarted: (message: string) => void;
  /** The server refused (nothing to download, too many files) or could not be reached. */
  onError: (error: unknown) => void;
  /** Tests replace the form post. */
  submit?: (body: Record<string, unknown>) => void;
}

/** "Download" for a whole album or the selection bar: one ZIP of the originals, streamed by the browser. */
export function ZipButton({ target, name, disabled, className = "btn btn-quiet", onStarted, onError, submit = submitZip }: Props) {
  const [busy, setBusy] = useState(false);

  async function start() {
    if (!isScope(target) && target.length > MAX_ZIP_IDS) {
      onError(new Error(`Pick at most ${MAX_ZIP_IDS.toLocaleString("en-US")} photos by hand, or use Select all.`));
      return;
    }
    setBusy(true);
    try {
      const body = zipBody(target, name);
      const plan = await zipApi.plan(body);
      submit(body);
      onStarted(zipStartedMessage(plan));
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <button
      type="button"
      className={className}
      disabled={disabled || busy}
      title="Download the originals as one ZIP"
      onClick={() => void start()}
    >
      {busy ? "Preparing…" : "Download"}
    </button>
  );
}
