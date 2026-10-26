import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { detail, item } from "../test/fixtures";
import { Lightbox } from "./Lightbox";
import { RateTools } from "./RateTools";
import { RatingStars } from "./Stars";

afterEach(() => vi.restoreAllMocks());

describe("RatingStars", () => {
  it("marks the current rating and chooses another, or clears with a second tap on the same star", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(<RatingStars value={3} onChange={onChange} label="Rating of a.jpg" />);
    expect(screen.getByRole("group", { name: "Rating of a.jpg: 3 stars" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^3 stars, selected/ })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: "5 stars" }));
    expect(onChange).toHaveBeenLastCalledWith(5);
    await user.click(screen.getByRole("button", { name: /^3 stars, selected/ }));
    expect(onChange).toHaveBeenLastCalledWith(0);
    rerender(<RatingStars value={undefined} onChange={onChange} />);
    expect(screen.getByRole("group", { name: "Rating: Not rated" })).toBeInTheDocument();
  });

  it("is a plain picture when nothing can be changed", () => {
    render(<RatingStars value={2} />);
    expect(screen.getByRole("img", { name: "2 stars" })).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});

describe("Lightbox ratings", () => {
  const items = [1, 2].map((n) => item(n, { rating: n === 2 ? 4 : 0 }));
  const mount = (onRate?: (i: unknown, n: number) => void, index = 1) =>
    render(
      <Lightbox
        items={items}
        index={index}
        onIndexChange={() => {}}
        onClose={() => {}}
        loadDetail={async (id) => detail(Number(id.split("-")[1]))}
        onRate={onRate}
      />,
    );

  it("rates the open photo with the keys 0 to 5 and with the stars under it", async () => {
    const user = userEvent.setup();
    const onRate = vi.fn();
    mount(onRate);
    await user.keyboard("3");
    expect(onRate).toHaveBeenLastCalledWith(items[1], 3);
    await user.keyboard("0");
    expect(onRate).toHaveBeenLastCalledWith(items[1], 0);
    await user.keyboard("6");
    await user.keyboard("a");
    expect(onRate).toHaveBeenCalledTimes(2);
    // The stars show the photo's own rating; tapping its star clears it.
    expect(screen.getByRole("group", { name: "Rating of photo-2.jpg: 4 stars" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^4 stars, selected/ }));
    expect(onRate).toHaveBeenLastCalledWith(items[1], 0);
    await user.click(screen.getByRole("button", { name: "2 stars" }));
    expect(onRate).toHaveBeenLastCalledWith(items[1], 2);
  });

  it("ignores digits with a modifier, repeats, and does nothing unless wired up", async () => {
    const user = userEvent.setup();
    const onRate = vi.fn();
    const { unmount } = mount(onRate);
    await user.keyboard("{Control>}3{/Control}");
    await user.keyboard("{Meta>}3{/Meta}");
    expect(onRate).not.toHaveBeenCalled();
    unmount();
    mount(undefined);
    await user.keyboard("3");
    expect(screen.queryByRole("group", { name: /Rating of/ })).not.toBeInTheDocument();
  });
});

describe("RateTools", () => {
  it("rates the selection with the chosen stars and tells the page", async () => {
    const user = userEvent.setup();
    const rate = vi.spyOn(api, "rateAssets").mockResolvedValue({ count: 3 });
    const onDone = vi.fn();
    render(<RateTools target={["a", "b", "c"]} count={3} onDone={onDone} onError={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Rate" }));
    const dialog = screen.getByRole("dialog", { name: "Rate photos" });
    expect(dialog).toHaveTextContent("3 photos");
    await user.click(screen.getByRole("button", { name: /4 stars/ }));
    expect(rate).toHaveBeenCalledWith(["a", "b", "c"], 4);
    await waitFor(() => expect(onDone).toHaveBeenCalledWith(4, 3, "3 photos rated 4 stars"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("clears ratings, sends a whole-view scope as it is, and reports a failure", async () => {
    const user = userEvent.setup();
    const rate = vi.spyOn(api, "rateAssets").mockResolvedValueOnce({ count: 9 }).mockRejectedValueOnce(new Error("boom"));
    const onDone = vi.fn();
    const onError = vi.fn();
    const target = { scope: "library" as const, except: ["x"] };
    render(<RateTools target={target} count={9} onDone={onDone} onError={onError} />);
    await user.click(screen.getByRole("button", { name: "Rate" }));
    await user.click(screen.getByRole("button", { name: /Clear rating/ }));
    expect(rate).toHaveBeenLastCalledWith(target, 0);
    await waitFor(() => expect(onDone).toHaveBeenCalledWith(0, 9, "Rating cleared on 9 photos"));
    await user.click(screen.getByRole("button", { name: "Rate" }));
    await user.click(screen.getByRole("button", { name: /5 stars/ }));
    await waitFor(() => expect(onError).toHaveBeenCalledOnce());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("closes with Escape or its Close button and rates nothing", async () => {
    const user = userEvent.setup();
    const rate = vi.spyOn(api, "rateAssets");
    render(<RateTools target={["a"]} count={1} onDone={() => {}} onError={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Rate" }));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Rate" }));
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(rate).not.toHaveBeenCalled();
  });
});
