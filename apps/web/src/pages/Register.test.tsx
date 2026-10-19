import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type RegistrationInfo } from "../api";
import { AuthProvider } from "../auth";
import { Register } from "./Register";

const info = (over: Partial<RegistrationInfo> = {}): RegistrationInfo => ({
  mode: "invite",
  first_account: false,
  open: false,
  invite_required: true,
  ...over,
});

beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(api, "me").mockRejectedValue(new ApiError(401, "unauthorized"));
});
afterEach(() => vi.restoreAllMocks());

const renderAt = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <AuthProvider>
        <Register />
      </AuthProvider>
    </MemoryRouter>,
  );

async function fill(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText("Email"), "sam@example.com");
  await user.type(screen.getByLabelText("Password"), "correct horse battery");
}

describe("Register page", () => {
  it("sends the invite from the link and checks it first", async () => {
    vi.spyOn(api, "registrationInfo").mockResolvedValue(info());
    const check = vi.spyOn(api, "checkInvite").mockResolvedValue({ valid: true, expires_at: "2099-01-01T00:00:00Z" });
    const register = vi.spyOn(api, "register").mockResolvedValue({ id: "u", email: "sam@example.com", role: "member" });
    const user = userEvent.setup();
    renderAt("/register?invite=abc123");
    expect(await screen.findByText("You have been invited. Create your account below.")).toBeInTheDocument();
    expect(check).toHaveBeenCalledWith("abc123");
    // The token came with the link: no field for it.
    expect(screen.queryByLabelText("Invite link or code")).not.toBeInTheDocument();
    await fill(user);
    await user.click(screen.getByRole("button", { name: "Create account" }));
    await waitFor(() => expect(register).toHaveBeenCalledWith("sam@example.com", "correct horse battery", "abc123"));
  });

  it.each([
    ["used", "already used"],
    ["revoked", "was revoked"],
    ["expired", "has expired"],
    ["unknown", "not valid"],
  ] as const)("says so up front when the invite is %s", async (reason, text) => {
    vi.spyOn(api, "registrationInfo").mockResolvedValue(info());
    vi.spyOn(api, "checkInvite").mockResolvedValue({ valid: false, reason });
    renderAt("/register?invite=abc123");
    expect(await screen.findByText(new RegExp(text))).toBeInTheDocument();
  });

  it("asks for an invite on an invite-only server, and takes a pasted link", async () => {
    vi.spyOn(api, "registrationInfo").mockResolvedValue(info());
    const register = vi.spyOn(api, "register").mockResolvedValue({ id: "u", email: "sam@example.com" });
    const user = userEvent.setup();
    renderAt("/register");
    const field = await screen.findByLabelText("Invite link or code");
    await fill(user);
    await user.click(field);
    await user.paste("https://photos.example/register?invite=tok999");
    await user.click(screen.getByRole("button", { name: "Create account" }));
    await waitFor(() => expect(register).toHaveBeenCalledWith("sam@example.com", "correct horse battery", "tok999"));
  });

  it("offers no invite field to the first account, and says it becomes the administrator", async () => {
    vi.spyOn(api, "registrationInfo").mockResolvedValue(info({ first_account: true, open: true, invite_required: false }));
    renderAt("/register");
    expect(await screen.findByText(/The first one becomes the administrator/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Invite link or code")).not.toBeInTheDocument();
  });

  it("explains a closed server", async () => {
    vi.spyOn(api, "registrationInfo").mockResolvedValue(info({ mode: "closed", invite_required: false }));
    renderAt("/register");
    expect(await screen.findByText(/Registration is closed on this server/)).toBeInTheDocument();
  });

  it("shows the server's reason when an invite is refused at submit", async () => {
    vi.spyOn(api, "registrationInfo").mockResolvedValue(info());
    vi.spyOn(api, "checkInvite").mockResolvedValue({ valid: true });
    vi.spyOn(api, "register").mockRejectedValue(new ApiError(403, "this invite link was already used"));
    const user = userEvent.setup();
    renderAt("/register?invite=abc123");
    await screen.findByText("You have been invited. Create your account below.");
    await fill(user);
    await user.click(screen.getByRole("button", { name: "Create account" }));
    expect(await screen.findByText("This invite link was already used.")).toBeInTheDocument();
    // An invite problem is not "registration is closed": the login page keeps offering the link.
    expect(sessionStorage.getItem("cyanotype.registration-closed")).toBeNull();
  });

  it("falls back to the old behaviour on a server without the registration endpoint", async () => {
    vi.spyOn(api, "registrationInfo").mockRejectedValue(new ApiError(404, "not found"));
    vi.spyOn(api, "register").mockRejectedValue(new ApiError(403, "registration is closed"));
    const user = userEvent.setup();
    renderAt("/register");
    await fill(user);
    await user.click(screen.getByRole("button", { name: "Create account" }));
    expect(await screen.findByText("Registration is closed.")).toBeInTheDocument();
  });
});
