// "Download as ZIP": the browser, not the page, receives the archive. The page asks the server what the archive would hold
// (so "nothing to download" or "too many files" show up here, not as a failed download), then starts an ordinary download
// the browser streams to disk with its own progress bar (the response has an exact Content-Length). Nothing is built in JS memory.
import type { ZipPlan } from "./api";
import { bytesLabel } from "./adminText";
import type { PublicZipPlan } from "./shareApi";

/** "Downloading 12 files (48 MB) as a ZIP." */
export function zipStartedMessage(plan: Pick<ZipPlan, "files" | "bytes">): string {
  const n = plan.files.toLocaleString("en-US");
  return `Downloading ${n} ${plan.files === 1 ? "file" : "files"} (${bytesLabel(plan.bytes)}) as a ZIP. Your browser shows the progress.`;
}

/**
 * Start the download of a selection: a hidden form posts it to the archive endpoint and the response (an attachment) goes to
 * disk without leaving the page. A form, not fetch(): fetch would hold the whole archive in memory.
 */
export function submitZip(body: Record<string, unknown>, doc: Document = document): void {
  const form = doc.createElement("form");
  form.method = "post";
  form.action = "/assets/zip";
  form.style.display = "none";
  const field = doc.createElement("input");
  field.type = "hidden";
  field.name = "selection";
  field.value = JSON.stringify(body);
  form.append(field);
  doc.body.append(form);
  form.submit();
  form.remove();
}

/** Start the download of the file at `href` (a plain GET the browser saves). */
export function followLink(href: string, doc: Document = document): void {
  const a = doc.createElement("a");
  a.href = href;
  a.download = "";
  a.rel = "noopener";
  a.style.display = "none";
  doc.body.append(a);
  a.click();
  a.remove();
}

/** How often and how long a share page asks whether the download copies are ready. */
export const PREPARE_MS = 1500;
export const PREPARE_TRIES = 40;

/** What "Download all" on a share page says once it starts. */
export function sharePlanMessage(plan: Pick<PublicZipPlan, "files" | "bytes" | "skipped">): string {
  const base = zipStartedMessage(plan);
  return plan.skipped > 0
    ? `${base} ${plan.skipped.toLocaleString("en-US")} ${plan.skipped === 1 ? "photo is" : "photos are"} not available to download.`
    : base;
}

/**
 * Wait until every download copy of a link is made (the server queues them as a side effect of each ask), then return the plan.
 * `sleep` is injectable for tests; `onWaiting` is told how many are still pending.
 */
export async function planShareZip(
  ask: () => Promise<PublicZipPlan>,
  opts: { sleep?: (ms: number) => Promise<void>; tries?: number; onWaiting?: (pending: number) => void } = {},
): Promise<PublicZipPlan> {
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const tries = opts.tries ?? PREPARE_TRIES;
  for (let i = 0; i < tries; i++) {
    const plan = await ask();
    if (plan.pending === 0) return plan;
    opts.onWaiting?.(plan.pending);
    await sleep(PREPARE_MS);
  }
  throw new Error("The download is still being prepared. Try again in a minute.");
}
