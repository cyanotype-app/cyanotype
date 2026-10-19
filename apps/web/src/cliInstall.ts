/** The one-line installs for the `cyanotype` command line, served by this very server under /cli (docs/releases.md). */
export type CliOs = "windows" | "unix";

export function detectOs(userAgent: string): CliOs {
  return /windows/i.test(userAgent) ? "windows" : "unix";
}

/** Commands for a server at `origin` (e.g. https://photos.example.com, no trailing slash). */
export function installCommands(origin: string): Record<CliOs, string> {
  const base = origin.replace(/\/+$/, "");
  return {
    unix: `curl -fsSL ${base}/cli/install.sh | sh`,
    windows: `irm ${base}/cli/install.ps1 | iex`,
  };
}

export const OS_LABEL: Record<CliOs, string> = { unix: "macOS and Linux", windows: "Windows (PowerShell)" };
