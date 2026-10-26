import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { api, type TwoFactorStatus } from "../api";
import { AuthProvider } from "../auth";
import { Security } from "./Security";
import { Login } from "./Login";
import { TwoFactorAdmin } from "./admin/TwoFactorAdmin";

const status = (over: Partial<TwoFactorStatus> = {}): TwoFactorStatus => ({
  available: true,
  enabled: false,
  recovery_codes_remaining: 0,
  required: false,
  ...over,
});

const me = { id: "u1", email: "a@example.com", role: "admin" as const };

function shell(ui: React.ReactElement) {
  return render(
    <MemoryRouter>
      <AuthProvider>{ui}</AuthProvider>
    </MemoryRouter>,
  );
}

beforeAll(() => {
  URL.createObjectURL = vi.fn(() => "blob:codes");
});
afterEach(() => vi.restoreAllMocks());

describe("Security", () => {
  it("says so when the server cannot do it", async () => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "twoFactorStatus").mockResolvedValue(status({ available: false }));
    shell(<Security />);
    expect(await screen.findByText(/not available on this server yet/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Set up/ })).not.toBeInTheDocument();
  });

  it("enrols: QR and key, a code, then the recovery codes once, gated on 'I have saved them'", async () => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    const st = vi
      .spyOn(api, "twoFactorStatus")
      .mockResolvedValueOnce(status())
      .mockResolvedValue(status({ enabled: true, recovery_codes_remaining: 10 }));
    vi.spyOn(api, "twoFactorEnrol").mockResolvedValue({
      secret: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ",
      otpauth_uri: "otpauth://totp/Cyanotype:a%40example.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Cyanotype",
      issuer: "Cyanotype",
      account: "a@example.com",
    });
    const confirm = vi.spyOn(api, "twoFactorConfirm").mockResolvedValue({
      enabled: true,
      recovery_codes: ["aaaa-bbbb-cccc-dddd", "eeee-ffff-gggg-hhhh"],
    });
    const user = userEvent.setup();
    shell(<Security />);
    await user.click(await screen.findByRole("button", { name: /Set up two-factor/ }));
    expect(screen.getByRole("img", { name: /QR code/ })).toBeInTheDocument();
    expect(screen.getByLabelText("Setup key")).toHaveTextContent("GEZD GNBV GY3T QOJQ GEZD GNBV GY3T QOJQ");
    await user.type(screen.getByLabelText(/6-digit code/), "123 456");
    await user.click(screen.getByRole("button", { name: "Turn on" }));
    expect(confirm).toHaveBeenCalledWith("123456");
    expect(await screen.findByText("aaaa-bbbb-cccc-dddd")).toBeInTheDocument();
    expect(screen.getByText(/will not be shown again/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Download" })).toHaveAttribute("download", "cyanotype-recovery-codes.txt");
    const done = screen.getByRole("button", { name: "Done" });
    expect(done).toBeDisabled();
    await user.click(screen.getByLabelText(/I have saved these codes/));
    await user.click(done);
    expect(screen.queryByText("aaaa-bbbb-cccc-dddd")).not.toBeInTheDocument();
    await waitFor(() => expect(st).toHaveBeenCalledTimes(2));
    expect(await screen.findByText(/10 of 10 recovery codes left/)).toBeInTheDocument();
  });

  it("goes back to the page that sent the person here once the recovery codes are saved", async () => {
    vi.spyOn(api, "me")
      .mockResolvedValueOnce({ ...me, mfa_enrol_required: true })
      .mockResolvedValue(me);
    vi.spyOn(api, "twoFactorStatus")
      .mockResolvedValueOnce(status({ required: true }))
      .mockResolvedValue(status({ enabled: true, recovery_codes_remaining: 10, required: true }));
    vi.spyOn(api, "twoFactorEnrol").mockResolvedValue({ secret: "GEZDGNBVGY3TQOJQ", otpauth_uri: "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ", issuer: "Cyanotype", account: "a@example.com" });
    vi.spyOn(api, "twoFactorConfirm").mockResolvedValue({ enabled: true, recovery_codes: ["aaaa-bbbb-cccc-dddd"] });
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={[{ pathname: "/settings/security", state: { from: "/activate#code=WDJB-MJHT" } }]}>
        <AuthProvider>
          <Routes>
            <Route path="/settings/security" element={<Security />} />
            <Route path="/activate" element={<p>activate page</p>} />
          </Routes>
        </AuthProvider>
      </MemoryRouter>,
    );
    await user.click(await screen.findByRole("button", { name: /Set up two-factor/ }));
    await user.type(screen.getByLabelText(/6-digit code/), "123456");
    await user.click(screen.getByRole("button", { name: "Turn on" }));
    expect(await screen.findByText("aaaa-bbbb-cccc-dddd")).toBeInTheDocument();
    // Not before the codes are saved.
    expect(screen.queryByText("activate page")).not.toBeInTheDocument();
    await user.click(screen.getByLabelText(/I have saved these codes/));
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(await screen.findByText("activate page")).toBeInTheDocument();
  });

  it("does not follow a return address that leaves the site", async () => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "twoFactorStatus")
      .mockResolvedValueOnce(status())
      .mockResolvedValue(status({ enabled: true, recovery_codes_remaining: 10 }));
    vi.spyOn(api, "twoFactorEnrol").mockResolvedValue({ secret: "GEZDGNBVGY3TQOJQ", otpauth_uri: "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ", issuer: "Cyanotype", account: "a@example.com" });
    vi.spyOn(api, "twoFactorConfirm").mockResolvedValue({ enabled: true, recovery_codes: ["aaaa-bbbb-cccc-dddd"] });
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={[{ pathname: "/settings/security", state: { from: "//evil.example/x" } }]}>
        <AuthProvider>
          <Routes>
            <Route path="/settings/security" element={<Security />} />
            <Route path="*" element={<p>somewhere else</p>} />
          </Routes>
        </AuthProvider>
      </MemoryRouter>,
    );
    await user.click(await screen.findByRole("button", { name: /Set up two-factor/ }));
    await user.type(screen.getByLabelText(/6-digit code/), "123456");
    await user.click(screen.getByRole("button", { name: "Turn on" }));
    await screen.findByText("aaaa-bbbb-cccc-dddd");
    await user.click(screen.getByLabelText(/I have saved these codes/));
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByText("somewhere else")).not.toBeInTheDocument();
    expect(await screen.findByText(/10 of 10 recovery codes left/)).toBeInTheDocument();
  });

  it("a wrong confirmation code shows the reason and keeps the setup open", async () => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "twoFactorStatus").mockResolvedValue(status());
    vi.spyOn(api, "twoFactorEnrol").mockResolvedValue({ secret: "ABCDEFGH", otpauth_uri: "otpauth://totp/x?secret=ABCDEFGH", issuer: "C", account: "a" });
    const { ApiError } = await import("../api");
    vi.spyOn(api, "twoFactorConfirm").mockRejectedValue(new ApiError(401, "That code is not right.", "invalid_code"));
    const user = userEvent.setup();
    shell(<Security />);
    await user.click(await screen.findByRole("button", { name: /Set up two-factor/ }));
    await user.type(screen.getByLabelText(/6-digit code/), "000000");
    await user.click(screen.getByRole("button", { name: "Turn on" }));
    expect(await screen.findByText(/That code is not right/)).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /QR code/ })).toBeInTheDocument();
  });

  it("turning it off asks for the password and a code", async () => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "twoFactorStatus").mockResolvedValue(status({ enabled: true, recovery_codes_remaining: 2 }));
    const off = vi.spyOn(api, "twoFactorDisable").mockResolvedValue({ enabled: false, enrol_required: false });
    const user = userEvent.setup();
    shell(<Security />);
    expect(await screen.findByText(/Make new ones soon/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Turn off" }));
    await user.type(screen.getByLabelText("Password"), "hunter22hunter");
    await user.type(screen.getByLabelText(/Code from the app/), "654 321");
    await user.click(within(screen.getByRole("form", { name: "Turn off two-factor authentication" })).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(off).toHaveBeenCalledWith("hunter22hunter", "654321"));
  });
});

describe("Login with a second factor", () => {
  it("asks for the code after the password and signs in with it", async () => {
    vi.spyOn(api, "me").mockRejectedValueOnce(Object.assign(new Error("x"), { status: 401 }));
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "login").mockResolvedValue({ mfa_required: true, challenge: "chal" });
    const code = vi.spyOn(api, "loginCode").mockResolvedValue({ id: "u1" });
    const user = userEvent.setup();
    shell(<Login />);
    await user.type(await screen.findByLabelText("Email"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "correct horse");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    const input = await screen.findByLabelText(/Code from your authenticator app/);
    await user.type(input, "123456");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(code).toHaveBeenCalledWith("chal", "123456"));
  });

  it("sends an account that must enrol to the set-up page, keeping where it was headed", async () => {
    vi.spyOn(api, "me").mockRejectedValueOnce(Object.assign(new Error("x"), { status: 401 }));
    vi.spyOn(api, "me").mockResolvedValue({ ...me, mfa_enrol_required: true });
    vi.spyOn(api, "login").mockResolvedValue({ id: "u1", mfa_enrol_required: true });
    function Here() {
      const loc = useLocation();
      return <p>security page, back to {(loc.state as { from?: string } | null)?.from}</p>;
    }
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={[{ pathname: "/login", state: { from: "/activate#code=WDJB-MJHT" } }]}>
        <AuthProvider>
          <Routes>
            <Route path="/login" element={<Login />} />
            <Route path="/settings/security" element={<Here />} />
          </Routes>
        </AuthProvider>
      </MemoryRouter>,
    );
    await user.type(await screen.findByLabelText("Email"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "correct horse");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("security page, back to /activate#code=WDJB-MJHT")).toBeInTheDocument();
  });

  it("offers a recovery code instead", async () => {
    vi.spyOn(api, "me").mockRejectedValue(Object.assign(new Error("x"), { status: 401 }));
    vi.spyOn(api, "login").mockResolvedValue({ mfa_required: true, challenge: "chal" });
    const user = userEvent.setup();
    shell(<Login />);
    await user.type(await screen.findByLabelText("Email"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "correct horse");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    await user.click(await screen.findByRole("button", { name: /Use a recovery code/ }));
    expect(screen.getByLabelText("Recovery code")).toBeInTheDocument();
  });
});

describe("Admin two-factor page", () => {
  const overview = (over = {}) => ({
    key_configured: true,
    required: false,
    enforced: false,
    users: [
      { id: "u1", email: "a@example.com", role: "admin" as const, enabled: true, since: "2026-01-01T00:00:00Z", recovery_codes_remaining: 10 },
      { id: "u2", email: "b@example.com", role: "member" as const, enabled: true, since: "2026-01-02T00:00:00Z", recovery_codes_remaining: 4 },
      { id: "u3", email: "c@example.com", role: "member" as const, enabled: false, since: null, recovery_codes_remaining: 0 },
    ],
    ...over,
  });

  it("warns plainly when the server key is missing", async () => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "adminTwoFactor").mockResolvedValue(overview({ key_configured: false }));
    shell(<TwoFactorAdmin />);
    expect(await screen.findByText(/switched off on this server/)).toBeInTheDocument();
    expect(screen.getByLabelText(/Require two-factor/)).toBeDisabled();
  });

  it("lists who has it on, requires it, and resets someone else's after a confirmation", async () => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "adminTwoFactor").mockResolvedValue(overview());
    const req = vi.spyOn(api, "adminRequireTwoFactor").mockResolvedValue({ required: true });
    const reset = vi.spyOn(api, "adminResetTwoFactor").mockResolvedValue({ reset: true, had_2fa: true });
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const user = userEvent.setup();
    shell(<TwoFactorAdmin />);
    expect(await screen.findByText(/2 of 3 have it on/)).toBeInTheDocument();
    // Only other accounts that have it can be reset (not yourself, not accounts without it).
    expect(screen.getAllByRole("button", { name: "Reset" })).toHaveLength(1);
    await user.click(screen.getByLabelText(/Require two-factor/));
    expect(req).toHaveBeenCalledWith(true);
    await user.click(screen.getByRole("button", { name: "Reset" }));
    await waitFor(() => expect(reset).toHaveBeenCalledWith("u2"));
  });
});
