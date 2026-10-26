// Signing the command line in through the browser: the pages /cli/authorize and /activate, and the small rules behind them.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "../api";
import { AuthProvider } from "../auth";
import { authorizeParams, codeFromHash, formatUserCode, isLoopbackRedirect, nav, userCodeComplete } from "../cliLoginText";
import { Activate } from "./Activate";
import { CliAuthorize } from "./CliAuthorize";

afterEach(() => vi.restoreAllMocks());

const QUERY =
  "?redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback&state=st-1&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&device=test-laptop&scope=import";
const PARAMS = {
  redirect_uri: "http://127.0.0.1:53682/callback",
  state: "st-1",
  code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
  device: "test-laptop",
  scope: "import",
};

const signedIn = () => vi.spyOn(api, "me").mockResolvedValue({ id: "u1", email: "alpha@example.com", role: "member" });

function LoginProbe() {
  const loc = useLocation();
  return <p>login page, back to {(loc.state as { from?: string } | null)?.from}</p>;
}

function SecurityProbe() {
  const loc = useLocation();
  return <p>security page, back to {(loc.state as { from?: string } | null)?.from}</p>;
}

function page(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <Routes>
          <Route path="/cli/authorize" element={<CliAuthorize />} />
          <Route path="/activate" element={<Activate />} />
          <Route path="/login" element={<LoginProbe />} />
          <Route path="/settings/security" element={<SecurityProbe />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe("the rules", () => {
  it("only an exact loopback callback is followed", () => {
    expect(isLoopbackRedirect("http://127.0.0.1:53682/callback?code=abc&state=s-1")).toBe(true);
    expect(isLoopbackRedirect("http://[::1]:8123/callback?error=access_denied&state=s")).toBe(true);
    for (const bad of [
      "https://evil.example/callback",
      "http://localhost:53682/callback",
      "http://127.0.0.1@evil.example/callback",
      "http://127.0.0.1:53682@evil.example/callback",
      "http://127.0.0.1:80/callback",
      "http://127.0.0.1:99999/callback",
      "http://127.0.0.1:53682/other?code=a",
      "http://127.0.0.1:53682/callback#x",
      "javascript:alert(1)",
      "",
    ]) {
      expect(isLoopbackRedirect(bad), bad).toBe(false);
    }
  });

  it("reads the request out of the address and formats the code as it is typed", () => {
    expect(authorizeParams(QUERY)).toEqual(PARAMS);
    expect(authorizeParams("?state=x")).toBeNull();
    expect(authorizeParams("")).toBeNull();
    expect(formatUserCode("wdjbm")).toBe("WDJB-M");
    expect(formatUserCode("wdjb-mjhtxyz")).toBe("WDJB-MJHT");
    expect(formatUserCode("w d1j2")).toBe("WDJ");
    expect(userCodeComplete("WDJB-MJHT")).toBe(true);
    expect(userCodeComplete("WDJB-MJH")).toBe(false);
  });

  it("reads a code out of the fragment of a scanned address, and nothing else", () => {
    expect(codeFromHash("#code=WDJB-MJHT")).toBe("WDJB-MJHT");
    expect(codeFromHash("#code=wdjbmjht")).toBe("WDJB-MJHT");
    expect(codeFromHash("#code=WDJB")).toBe("");
    expect(codeFromHash("#other=WDJB-MJHT")).toBe("");
    expect(codeFromHash("")).toBe("");
  });
});

describe("CliAuthorize", () => {
  it("sends an account that must set up two-factor authentication there, and remembers the request", async () => {
    vi.spyOn(api, "me").mockResolvedValue({ id: "u1", email: "alpha@example.com", role: "member", mfa_enrol_required: true });
    page(`/cli/authorize${QUERY}`);
    expect(await screen.findByText(`security page, back to /cli/authorize${QUERY}`)).toBeInTheDocument();
  });

  it("asks anonymous visitors to sign in first and brings them back to the same request", async () => {
    vi.spyOn(api, "me").mockRejectedValue(new ApiError(401, "unauthorized"));
    page(`/cli/authorize${QUERY}`);
    expect(await screen.findByText(`login page, back to /cli/authorize${QUERY}`)).toBeInTheDocument();
  });

  it("names the device, the access and the account, and Allow follows the loopback redirect", async () => {
    signedIn();
    const check = vi.spyOn(api, "cliAuthorizeCheck").mockResolvedValue({ device: "test-laptop", scope: "import" });
    const approve = vi
      .spyOn(api, "cliAuthorizeApprove")
      .mockResolvedValue({ redirect: "http://127.0.0.1:53682/callback?code=abc&state=st-1" });
    const go = vi.spyOn(nav, "to").mockImplementation(() => {});
    const user = userEvent.setup();
    page(`/cli/authorize${QUERY}`);
    expect(await screen.findByText("Allow cyanotype CLI on test-laptop to access your library?")).toBeInTheDocument();
    expect(check).toHaveBeenCalledWith(PARAMS);
    expect(screen.getByText("Import only")).toBeInTheDocument();
    expect(screen.getByText("alpha@example.com")).toBeInTheDocument();
    expect(go).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(go).toHaveBeenCalledWith("http://127.0.0.1:53682/callback?code=abc&state=st-1"));
    expect(approve).toHaveBeenCalledWith(PARAMS);
    expect(await screen.findByText(/You can close this tab/)).toBeInTheDocument();
  });

  it("Deny reports back to the command line without allowing anything", async () => {
    signedIn();
    vi.spyOn(api, "cliAuthorizeCheck").mockResolvedValue({ device: "test-laptop", scope: "import" });
    const approve = vi.spyOn(api, "cliAuthorizeApprove");
    vi.spyOn(api, "cliAuthorizeDeny").mockResolvedValue({
      redirect: "http://127.0.0.1:53682/callback?error=access_denied&state=st-1",
    });
    const go = vi.spyOn(nav, "to").mockImplementation(() => {});
    const user = userEvent.setup();
    page(`/cli/authorize${QUERY}`);
    await user.click(await screen.findByRole("button", { name: "Deny" }));
    await waitFor(() => expect(go).toHaveBeenCalledWith("http://127.0.0.1:53682/callback?error=access_denied&state=st-1"));
    expect(approve).not.toHaveBeenCalled();
  });

  it("never follows a redirect that is not the loopback listener", async () => {
    signedIn();
    vi.spyOn(api, "cliAuthorizeCheck").mockResolvedValue({ device: "test-laptop", scope: "import" });
    vi.spyOn(api, "cliAuthorizeApprove").mockResolvedValue({ redirect: "https://evil.example/callback?code=abc" });
    const go = vi.spyOn(nav, "to").mockImplementation(() => {});
    const user = userEvent.setup();
    page(`/cli/authorize${QUERY}`);
    await user.click(await screen.findByRole("button", { name: "Allow" }));
    expect(await screen.findByText(/unexpected address/)).toBeInTheDocument();
    expect(go).not.toHaveBeenCalled();
  });

  it("explains a request the server refuses, and one that is not a login link at all", async () => {
    signedIn();
    vi.spyOn(api, "cliAuthorizeCheck").mockRejectedValue(new ApiError(400, "redirect_uri must be http://127.0.0.1:<port>/callback"));
    page(`/cli/authorize${QUERY}`);
    expect(await screen.findByText(/redirect_uri must be/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Allow" })).not.toBeInTheDocument();
  });

  it("is not a login link without the parameters", async () => {
    signedIn();
    const check = vi.spyOn(api, "cliAuthorizeCheck");
    page("/cli/authorize");
    expect(await screen.findByText(/not a login link/)).toBeInTheDocument();
    expect(check).not.toHaveBeenCalled();
  });
});

describe("Activate", () => {
  it("fills in the code a scanned QR code carries, and still asks before anything is allowed", async () => {
    signedIn();
    const lookup = vi.spyOn(api, "cliDeviceLookup").mockResolvedValue({ device: "test-laptop", scope: "import", expires_in: 500 });
    const approve = vi.spyOn(api, "cliDeviceApprove").mockResolvedValue({ approved: true, device: "test-laptop" });
    const user = userEvent.setup();
    page("/activate#code=WDJB-MJHT");
    expect(await screen.findByLabelText("Code")).toHaveValue("WDJB-MJHT");
    expect(lookup).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(lookup).toHaveBeenCalledWith("WDJB-MJHT");
    expect(await screen.findByRole("button", { name: "Allow" })).toBeInTheDocument();
    expect(approve).not.toHaveBeenCalled();
  });

  it("keeps the code through signing in and through two-factor set-up", async () => {
    vi.spyOn(api, "me").mockRejectedValue(new ApiError(401, "unauthorized"));
    const first = page("/activate#code=WDJB-MJHT");
    expect(await screen.findByText("login page, back to /activate#code=WDJB-MJHT")).toBeInTheDocument();
    first.unmount();

    vi.restoreAllMocks();
    vi.spyOn(api, "me").mockResolvedValue({ id: "u1", email: "alpha@example.com", role: "member", mfa_enrol_required: true });
    page("/activate#code=WDJB-MJHT");
    expect(await screen.findByText("security page, back to /activate#code=WDJB-MJHT")).toBeInTheDocument();
  });

  it("looks the typed code up, shows the device, and approves with the code in the body only", async () => {
    signedIn();
    const lookup = vi.spyOn(api, "cliDeviceLookup").mockResolvedValue({ device: "test-laptop", scope: "import", expires_in: 500 });
    const approve = vi.spyOn(api, "cliDeviceApprove").mockResolvedValue({ approved: true, device: "test-laptop" });
    const user = userEvent.setup();
    page("/activate");
    const input = await screen.findByLabelText("Code");
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    await user.type(input, "wdjbmjht");
    expect(input).toHaveValue("WDJB-MJHT");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(lookup).toHaveBeenCalledWith("WDJB-MJHT");
    expect(await screen.findByText("Allow cyanotype CLI on test-laptop to access your library?")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Allow" }));
    expect(approve).toHaveBeenCalledWith("WDJB-MJHT");
    expect(await screen.findByText(/command line on test-laptop is signing in/)).toBeInTheDocument();
  });

  it("shows the server's words for a wrong or expired code and allows another try", async () => {
    signedIn();
    vi.spyOn(api, "cliDeviceLookup").mockRejectedValue(new ApiError(404, "That code was not recognised, or it has expired.", "unknown_code"));
    const user = userEvent.setup();
    page("/activate");
    await user.type(await screen.findByLabelText("Code"), "BCDFGHJK");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByText(/was not recognised/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
  });

  it("Deny tells the server and says nothing was allowed", async () => {
    signedIn();
    vi.spyOn(api, "cliDeviceLookup").mockResolvedValue({ device: "test-laptop", scope: "read" });
    const deny = vi.spyOn(api, "cliDeviceDeny").mockResolvedValue({ approved: false, device: "test-laptop" });
    const user = userEvent.setup();
    page("/activate");
    await user.type(await screen.findByLabelText("Code"), "BCDFGHJK");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByText("Read only")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Deny" }));
    expect(deny).toHaveBeenCalledWith("BCDF-GHJK");
    expect(await screen.findByText(/test-laptop was not allowed/)).toBeInTheDocument();
  });
});
