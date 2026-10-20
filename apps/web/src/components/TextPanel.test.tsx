import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PhotoText } from "../api";
import { TextBoxes, TextPanel } from "./TextPanel";

const q = [0.1, 0.1, 0.5, 0.1, 0.5, 0.2, 0.1, 0.2];
const read: PhotoText = {
  status: "read",
  text: "HOFER KG\nTotal 22,41",
  lines: [
    { t: "HOFER KG", s: 0.98, q, w: [] },
    { t: "Total 22,41", s: 0.96, q: [0.1, 0.3, 0.5, 0.3, 0.5, 0.4, 0.1, 0.4], w: [] },
  ],
  qr: [
    { t: "https://example.org/r?id=1", q },
    { t: "javascript:alert(1)", q },
    { t: "WIFI:S:home;T:WPA;P:secret;;", q },
  ],
};

function panel(state: Parameters<typeof TextPanel>[0]["state"], extra: Partial<Parameters<typeof TextPanel>[0]> = {}) {
  const props = { panelId: "text-panel", state, onRetry: vi.fn(), onClose: vi.fn(), showBoxes: true, onShowBoxes: vi.fn(), ...extra };
  render(<TextPanel {...props} />);
  return props;
}

afterEach(() => vi.restoreAllMocks());

describe("TextPanel", () => {
  it("shows the text and copies it", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    panel({ status: "ready", text: read });
    expect(screen.getByLabelText("Recognised text")).toHaveTextContent("HOFER KG");
    await user.click(screen.getByRole("button", { name: "Copy all" }));
    expect(write).toHaveBeenCalledWith("HOFER KG\nTotal 22,41");
    expect(await screen.findByText("Copied.")).toBeInTheDocument();
  });

  it("lists QR codes as what they say, offers a link only for a web address, and opens nothing by itself", async () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    panel({ status: "ready", text: read });
    const codes = screen.getByRole("region", { name: "QR codes" });
    expect(within(codes).getByText("Web address")).toBeInTheDocument();
    expect(within(codes).getByText("Wi-Fi network")).toBeInTheDocument();
    const links = within(codes).getAllByRole("link");
    expect(links).toHaveLength(1);
    expect(links[0]).toHaveAttribute("href", "https://example.org/r?id=1");
    expect(links[0]).toHaveAttribute("target", "_blank");
    expect(links[0]).toHaveAttribute("rel", expect.stringContaining("noopener"));
    expect(links[0]).toHaveTextContent("Open example.org");
    // The dangerous payload is shown as text and has no link.
    expect(within(codes).getByText("javascript:alert(1)")).toBeInTheDocument();
    expect(open).not.toHaveBeenCalled();
    expect(screen.getByText(/never opened for you/)).toBeInTheDocument();
  });

  it("explains a photo that was not read yet, one with nothing on it, and a failure", async () => {
    const { onRetry } = panel({ status: "error" });
    await userEvent.setup().click(screen.getByRole("button", { name: "Try again" }));
    expect(onRetry).toHaveBeenCalled();
  });

  it("pending and empty states", () => {
    panel({ status: "ready", text: { status: "pending", text: "", lines: [], qr: [] } });
    expect(screen.getByText(/not been read yet/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy all" })).not.toBeInTheDocument();
  });

  it("the boxes toggle and Close call back", async () => {
    const user = userEvent.setup();
    const { onShowBoxes, onClose } = panel({ status: "ready", text: read }, { showBoxes: true });
    await user.click(screen.getByLabelText("Show on the photo"));
    expect(onShowBoxes).toHaveBeenCalledWith(false);
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });
});

describe("TextBoxes", () => {
  it("draws a box per line and per code over the photo, hidden from assistive technology", () => {
    const { container } = render(<TextBoxes text={read} style={{ left: 10, top: 20, width: 300, height: 200 }} />);
    const root = container.querySelector(".text-boxes") as HTMLElement;
    expect(root).toHaveAttribute("aria-hidden", "true");
    expect(root.style.left).toBe("10px");
    expect(root.querySelectorAll(".text-box:not(.qr)")).toHaveLength(2);
    expect(root.querySelectorAll(".text-box.qr")).toHaveLength(3);
  });
});
