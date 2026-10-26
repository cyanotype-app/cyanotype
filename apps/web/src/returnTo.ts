// Where to go once a detour (sign-in, two-factor set-up) is over. The place is kept in the router's state (never in an
// address a visitor could be sent to with a link), and is checked again here before anything follows it: a path on this site
// and nothing else, so no scheme, host or `//` can turn it into an open redirect.

/** `/activate#code=WDJB-MJHT`-style paths only: one leading slash, then address characters; no scheme, host, backslash or control character. */
const SAFE = /^\/(?!\/)[A-Za-z0-9\-._~!$&'()*+,;=:@%/?#]*$/;

/** The path when it is safe to navigate to inside the app, else null. */
export function safeReturnTo(raw: unknown): string | null {
  if (typeof raw !== "string" || raw.length > 2048 || !SAFE.test(raw)) return null;
  // Two slashes after a start such as `/\` or `/.//` are not possible here, but a path that climbs out is not worth keeping.
  return raw.split(/[?#]/, 1)[0]!.split("/").includes("..") ? null : raw;
}

/** The place a page was asked to come back to, from the router state `{ from }`. */
export function returnToOf(state: unknown): string | null {
  return safeReturnTo((state as { from?: unknown } | null)?.from);
}
