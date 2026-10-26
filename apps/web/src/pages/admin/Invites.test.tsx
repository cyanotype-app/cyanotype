import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type Invite, type IssuedInvite, type Me } from "../../api";
import { AuthProvider } from "../../auth";
import { AdminLayout } from "./AdminLayout";
import { Invites } from "./Invites";

const invite = (n: number, over: Partial<Invite> = {}): Invite => ({
  id: `i${n}`,
  note: `invite ${n}`,
  status: "pending",
  created_at: "2026-06-01T00:00:00Z",
  created_by: "admin@example.com",
  expires_at: "2099-01-01T00:00:00Z",
  revoked_at: null,
  used_at: null,
  used_by: null,
  limits: { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null },
  ...over,
});
const issued = (inv: Invite, token = "t0k3n"): IssuedInvite => ({ invite: inv, token, path: `/register?invite=${token}` });

const sure = async (user: ReturnType<typeof userEvent.setup>, name: string) =>
  user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name }));

afterEach(() => vi.restoreAllMocks());

describe("Invites page", () => {
  it("lists invites with their state and who used them", async () => {
    vi.spyOn(api, "listInvites").mockResolvedValue({
      items: [
        invite(1),
        invite(2, { status: "used", used_by: "sam@example.com", used_at: "2026-06-02T00:00:00Z", note: null }),
        invite(3, { status: "revoked" }),
        invite(4, { status: "expired" }),
        invite(5, { limits: { quota_bytes: 5 * 1024 ** 3, daily_upload_bytes: null, max_file_bytes: null } }),
      ],
    });
    render(<Invites />);
    expect(await screen.findByText("invite 1")).toBeInTheDocument();
    expect(screen.getByText("Used by sam@example.com")).toBeInTheDocument();
    expect(screen.getByText("No note")).toBeInTheDocument();
    expect(screen.getByText("Starts with 5.0 GB storage")).toBeInTheDocument();
    // Only unspent invites can be shown again or revoked.
    expect(screen.getAllByRole("button", { name: "Show link again" })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: "Revoke" })).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Renew link" })).toBeInTheDocument();
  });

  it("creates an invite with limits, shows the link once and copies it", async () => {
    vi.spyOn(api, "listInvites").mockResolvedValue({ items: [] });
    const create = vi.spyOn(api, "createInvite").mockResolvedValue(issued(invite(1), "SECRETTOKEN"));
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(<Invites />);
    await screen.findByText("No invites yet.");
    await user.type(screen.getByLabelText(/Note/), "for Sam");
    await user.selectOptions(screen.getByLabelText("Expires after"), "14");
    await user.click(screen.getByText("Starting limits (optional)"));
    await user.type(screen.getByLabelText("Storage (GB)"), "5");
    await user.type(screen.getByLabelText("Largest file (GB)"), "0.5");
    await user.click(screen.getByRole("button", { name: "Create invite" }));

    expect(create).toHaveBeenCalledWith({
      note: "for Sam",
      expires_in_days: 14,
      quota_bytes: 5 * 1024 ** 3,
      max_file_bytes: 1024 ** 3 / 2,
    });
    const box = await screen.findByRole("region", { name: "Invite link" });
    const link = `${window.location.origin}/register?invite=SECRETTOKEN`;
    expect(within(box).getByText(link)).toBeInTheDocument();
    await user.click(within(box).getByRole("button", { name: "Copy link" }));
    expect(writeText).toHaveBeenCalledWith(link);
    expect(await within(box).findByRole("button", { name: "Copied" })).toBeInTheDocument();
    await user.click(within(box).getByRole("button", { name: "Done" }));
    expect(screen.queryByText(link)).not.toBeInTheDocument();
  });

  it("refuses a limit that is not an amount, without calling the server", async () => {
    vi.spyOn(api, "listInvites").mockResolvedValue({ items: [] });
    const create = vi.spyOn(api, "createInvite");
    const user = userEvent.setup();
    render(<Invites />);
    await screen.findByText("No invites yet.");
    await user.click(screen.getByText("Starting limits (optional)"));
    await user.type(screen.getByLabelText("Storage (GB)"), "lots");
    await user.click(screen.getByRole("button", { name: "Create invite" }));
    expect(await screen.findByText(/greater than zero/)).toBeInTheDocument();
    expect(create).not.toHaveBeenCalled();
  });

  it("shows the link again by re-issuing after confirmation", async () => {
    vi.spyOn(api, "listInvites").mockResolvedValue({ items: [invite(1)] });
    const reissue = vi.spyOn(api, "reissueInvite").mockResolvedValue(issued(invite(1), "NEWTOKEN"));
    const user = userEvent.setup();
    render(<Invites />);
    await user.click(await screen.findByRole("button", { name: "Show link again" }));
    expect(screen.getByRole("alertdialog")).toHaveTextContent("the old link stops working");
    await sure(user, "Make a new link");
    expect(reissue).toHaveBeenCalledWith("i1");
    expect(await screen.findByText(`${window.location.origin}/register?invite=NEWTOKEN`)).toBeInTheDocument();
  });

  it("does nothing when the confirmation is declined", async () => {
    vi.spyOn(api, "listInvites").mockResolvedValue({ items: [invite(1)] });
    const reissue = vi.spyOn(api, "reissueInvite");
    const revoke = vi.spyOn(api, "revokeInvite");
    const user = userEvent.setup();
    render(<Invites />);
    await user.click(await screen.findByRole("button", { name: "Show link again" }));
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await user.click(screen.getByRole("button", { name: "Revoke" }));
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(reissue).not.toHaveBeenCalled();
    expect(revoke).not.toHaveBeenCalled();
  });

  it("revokes and reloads", async () => {
    const list = vi
      .spyOn(api, "listInvites")
      .mockResolvedValueOnce({ items: [invite(1)] })
      .mockResolvedValue({ items: [invite(1, { status: "revoked" })] });
    const revoke = vi.spyOn(api, "revokeInvite").mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<Invites />);
    await user.click(await screen.findByRole("button", { name: "Revoke" }));
    await sure(user, "Revoke");
    expect(revoke).toHaveBeenCalledWith("i1");
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Revoke" })).not.toBeInTheDocument());
  });

  it("reports an error from the server", async () => {
    vi.spyOn(api, "listInvites").mockResolvedValue({ items: [invite(1)] });
    vi.spyOn(api, "revokeInvite").mockRejectedValue(new ApiError(409, "this invite was already used"));
    const user = userEvent.setup();
    render(<Invites />);
    await user.click(await screen.findByRole("button", { name: "Revoke" }));
    await sure(user, "Revoke");
    expect(await screen.findByText("this invite was already used")).toBeInTheDocument();
  });
});

describe("AdminLayout", () => {
  const renderAt = (me: Me) => {
    vi.spyOn(api, "me").mockResolvedValue(me);
    vi.spyOn(api, "listInvites").mockResolvedValue({ items: [] });
    return render(
      <MemoryRouter initialEntries={["/admin/invites"]}>
        <AuthProvider>
          <Routes>
            <Route path="/admin" element={<AdminLayout />}>
              <Route path="invites" element={<Invites />} />
            </Route>
          </Routes>
        </AuthProvider>
      </MemoryRouter>,
    );
  };

  it("shows the invites to an admin", async () => {
    renderAt({ id: "u", email: "a@b.co", role: "admin" });
    expect(await screen.findByRole("heading", { name: "Invites" })).toBeInTheDocument();
    // jsdom has no wide screen: the phone layout, with the section as its own page and a way back.
    expect(screen.getByRole("heading", { level: 1, name: "Invites" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Administration/ })).toHaveAttribute("href", "/admin");
  });

  it("shows a member the page-not-found page, and never asks for the invites", async () => {
    renderAt({ id: "u", email: "m@b.co", role: "member" });
    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeInTheDocument();
    expect(api.listInvites).not.toHaveBeenCalled();
  });

  it("treats a server that reports no role as a member", async () => {
    renderAt({ id: "u", email: "m@b.co" });
    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeInTheDocument();
  });
});
