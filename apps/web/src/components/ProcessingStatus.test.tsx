import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Processing } from "../api";
import { nextPollMs, processingLabel, processingPercent } from "../processingText";
import { ProcessingStatus } from "./ProcessingStatus";

const base: Processing = {
  active: false,
  queued: 0,
  running: 0,
  failed: 0,
  by_kind: [],
  total_photos: 0,
  without_thumbnail: 0,
  failures: [],
};
const busy: Processing = { ...base, active: true, queued: 2400, running: 2, total_photos: 5000, without_thumbnail: 1234 };
const failing: Processing = {
  ...base,
  failed: 3,
  failures: [
    { job_id: 1, asset_id: "a1", original_name: "IMG_0001.CR3", kind: "thumbnail", last_error: "does not fit the worker memory budget", attempts: 5 },
    { job_id: 2, asset_id: "a2", original_name: "IMG_0002.HEIC", kind: "metadata", last_error: null, attempts: 1 },
  ],
};

function fakeApi(...answers: Processing[]) {
  let i = 0;
  return {
    processing: vi.fn(async () => answers[Math.min(i++, answers.length - 1)]!),
    retryFailedJobs: vi.fn(async () => ({ requeued: 3 })),
  };
}

beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
afterEach(() => vi.useRealTimers());

describe("ProcessingStatus", () => {
  it("renders nothing while idle", async () => {
    const api = fakeApi(base);
    const { container } = render(<ProcessingStatus api={api} />);
    await waitFor(() => expect(api.processing).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the number of photos waiting and a progress bar", async () => {
    render(<ProcessingStatus api={fakeApi(busy)} />);
    expect(await screen.findByText("Processing 1,234 photos…")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "75");
    expect(screen.queryByRole("button", { name: /failed/i })).not.toBeInTheDocument();
  });

  it("polls quickly while active and disappears when the work is done", async () => {
    const api = fakeApi(busy, busy, base);
    const { container } = render(<ProcessingStatus api={api} />);
    await screen.findByText(/Processing/);
    await act(() => vi.advanceTimersByTimeAsync(3100));
    expect(api.processing).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(3100));
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });

  it("backs off while idle", async () => {
    const api = fakeApi(base);
    render(<ProcessingStatus api={api} />);
    await waitFor(() => expect(api.processing).toHaveBeenCalledTimes(1));
    await act(() => vi.advanceTimersByTimeAsync(9000));
    expect(api.processing).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(api.processing).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(15_000));
    expect(api.processing).toHaveBeenCalledTimes(2); // the second wait is 20 s
  });

  it("keeps the last status when a poll fails", async () => {
    const api = fakeApi(busy);
    api.processing.mockResolvedValueOnce(busy).mockRejectedValueOnce(new Error("offline"));
    render(<ProcessingStatus api={api} />);
    await screen.findByText(/Processing/);
    await act(() => vi.advanceTimersByTimeAsync(3100));
    expect(screen.getByText(/Processing/)).toBeInTheDocument();
  });

  it("lists failures in a panel and retries them", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const api = fakeApi(failing, { ...busy, active: true });
    render(<ProcessingStatus api={api} />);
    const toggle = await screen.findByRole("button", { name: "3 failed" });
    expect(screen.queryByText("IMG_0001.CR3")).not.toBeInTheDocument();
    await user.click(toggle);
    expect(screen.getByText("IMG_0001.CR3")).toBeInTheDocument();
    expect(screen.getByText(/does not fit the worker memory budget/)).toBeInTheDocument();
    expect(screen.getByText(/Making thumbnail, 5 attempts/)).toBeInTheDocument();
    expect(screen.getByText(/Reading details, 1 attempt$/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry failed" }));
    expect(api.retryFailedJobs).toHaveBeenCalledTimes(1);
    // The status is re-read right away, so the queue shows up without waiting for the poll.
    expect(await screen.findByText("Processing 1,234 photos…")).toBeInTheDocument();
  });

  it("says so when retrying fails", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const api = fakeApi(failing);
    api.retryFailedJobs.mockRejectedValueOnce(new Error("500"));
    render(<ProcessingStatus api={api} />);
    await user.click(await screen.findByRole("button", { name: "3 failed" }));
    await user.click(screen.getByRole("button", { name: "Retry failed" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/couldn’t retry/i);
  });
});

describe("processing text", () => {
  it("labels", () => {
    expect(processingLabel({ ...base, without_thumbnail: 1 })).toBe("Processing 1 photo…");
    expect(processingLabel({ ...base, active: true, queued: 2, running: 1 })).toBe("Finishing 3 tasks…");
    expect(processingLabel(base)).toBe("");
  });
  it("percent", () => {
    expect(processingPercent({ ...base, total_photos: 10, without_thumbnail: 5 })).toBe(50);
    expect(processingPercent({ ...base, total_photos: 10, without_thumbnail: 0 })).toBeNull();
    expect(processingPercent({ ...base, total_photos: 1000, without_thumbnail: 1 })).toBe(99);
  });
  it("poll interval", () => {
    expect(nextPollMs(true, 5)).toBe(3000);
    expect(nextPollMs(false, 0)).toBe(10_000);
    expect(nextPollMs(false, 9)).toBe(60_000);
  });
});
