import { describe, expect, it } from "vitest";
import { detectOs, installCommands } from "./cliInstall";

describe("cliInstall", () => {
  it("picks the OS from the user agent", () => {
    expect(detectOs("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/130")).toBe("windows");
    expect(detectOs("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605")).toBe("unix");
    expect(detectOs("Mozilla/5.0 (X11; Linux x86_64) Firefox/131")).toBe("unix");
    expect(detectOs("")).toBe("unix");
  });

  it("builds the commands for this server", () => {
    expect(installCommands("https://photos.example.com")).toEqual({
      unix: "curl -fsSL https://photos.example.com/cli/install.sh | sh",
      windows: "irm https://photos.example.com/cli/install.ps1 | iex",
    });
    expect(installCommands("http://localhost:3000/").unix).toBe("curl -fsSL http://localhost:3000/cli/install.sh | sh");
  });
});
