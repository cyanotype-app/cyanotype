// The Content-Security-Policy the gateway sends for the web UI, read from its one source of truth, infra/caddy/csp.caddy,
// so the browser tests run under exactly what Caddy serves and the two cannot drift. `{$NAME:default}` placeholders are
// filled the way Caddy fills them (environment, else the default); `E2E_S3_ORIGIN` stands in for the bucket.
import { readFileSync } from "node:fs";

/** The origin presigned photo URLs point at in the tests (no server answers there: the specs fulfil it with page.route). */
export const TEST_S3_ORIGIN = "http://s3.csp.test:9000";

export function caddySnippetPolicy(name: "csp_spa" | "csp_api", env: Record<string, string> = {}): string {
  const file = readFileSync(new URL("../../../infra/caddy/csp.caddy", import.meta.url), "utf8");
  const block = new RegExp(`\\(${name}\\) \\{([\\s\\S]*?)\\n\\}`).exec(file)?.[1];
  const quoted = block && /import csp_header \{args\[0\]\} "([^"]+)"/.exec(block)?.[1];
  if (!quoted) throw new Error(`no ${name} policy found in infra/caddy/csp.caddy`);
  return quoted
    .replace(/\{\$([A-Z0-9_]+):([^}]*)\}/g, (_, v: string, dflt: string) => env[v] ?? dflt)
    .replace(/\{args\[\d+\]\}/g, "");
}

/** The web UI policy as enforced in the tests (and the extra directive Caddyfile.prod appends is left out: no TLS here). */
export const spaPolicy = () => caddySnippetPolicy("csp_spa", { CSP_S3_ORIGIN: TEST_S3_ORIGIN });
