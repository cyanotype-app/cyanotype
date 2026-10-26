// Words and small rules for signing the command line in through the browser (pages /cli/authorize and /activate). The server
// decides everything that matters; these only avoid a pointless round trip and keep the pages' wording in one place.

export interface CliAuthorizeParams {
  redirect_uri: string;
  state: string;
  code_challenge: string;
  device: string;
  scope: string;
}

/** The only places the page may send the browser after Allow or Deny: the CLI's own listener, `http://127.0.0.1:<port>/callback` or the IPv6 form. */
const LOOPBACK = /^http:\/\/(?:127\.0\.0\.1|\[::1\]):([1-9][0-9]{0,4})\/callback(?:\?[A-Za-z0-9=&._~-]*)?$/;

export function isLoopbackRedirect(uri: string): boolean {
  const port = LOOPBACK.exec(uri)?.[1];
  return port !== undefined && Number(port) >= 1024 && Number(port) <= 65535;
}

/** What the CLI put in the address (`?redirect_uri=…&state=…&code_challenge=…&device=…&scope=…`); null when it is not a login request. */
export function authorizeParams(search: string): CliAuthorizeParams | null {
  const q = new URLSearchParams(search);
  const redirect_uri = q.get("redirect_uri") ?? "";
  const state = q.get("state") ?? "";
  const code_challenge = q.get("code_challenge") ?? "";
  if (!redirect_uri || !state || !code_challenge) return null;
  return {
    redirect_uri,
    state,
    code_challenge,
    device: (q.get("device") ?? "").slice(0, 200),
    scope: q.get("scope") ?? "",
  };
}

/** The user code as it is typed: letters only, upper case, `WDJB-MJHT`. */
export function formatUserCode(raw: string): string {
  const letters = raw.replace(/[^A-Za-z]/g, "").toUpperCase().slice(0, 8);
  return letters.length > 4 ? `${letters.slice(0, 4)}-${letters.slice(4)}` : letters;
}

/** The code a scanned QR code carries: `/activate#code=WDJB-MJHT`. It is in the fragment, which the browser never sends to a server or a referrer. Empty unless it is a whole code. */
export function codeFromHash(hash: string): string {
  const raw = new URLSearchParams(hash.replace(/^#/, "")).get("code") ?? "";
  const code = formatUserCode(raw);
  return userCodeComplete(code) ? code : "";
}

export const userCodeComplete = (code: string): boolean => code.replace(/[^A-Za-z]/g, "").length === 8;

export const ALLOW_QUESTION = (device: string): string => `Allow cyanotype CLI on ${device} to access your library?`;

export const ONLY_IF_YOU_STARTED_IT =
  "Only continue if you just ran cyanotype login yourself on that computer. If you did not, choose Deny.";

/** Leaving the page for the CLI's listener; an object so a test can watch it. */
export const nav = { to: (url: string): void => window.location.assign(url) };

/** A redirect the server handed back that the page will not follow (it should never happen). */
export const BAD_REDIRECT = "The server sent an unexpected address, so nothing was followed. Close this tab and try again.";
