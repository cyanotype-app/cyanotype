import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type MyUsage } from "../api";
import { Storage } from "./Storage";

const GB = 1024 ** 3;
const usage = (over: Partial<MyUsage> = {}): MyUsage => ({
  originals_bytes: 3 * GB,
  trash_bytes: GB,
  stored_bytes: 4 * GB,
  derived_bytes: 200 * 1024 ** 2,
  derived_complete: true,
  photo_count: 1200,
  raw_count: 300,
  file_count: 1500,
  trash_count: 40,
  uploads: { count_24h: 12, bytes_24h: 600 * 1024 ** 2, count_30d: 900, bytes_30d: 2 * GB },
  ...over,
});

afterEach(() => vi.restoreAllMocks());
const renderPage = () =>
  render(
    <MemoryRouter>
      <Storage />
    </MemoryRouter>,
  );

describe("Storage page", () => {
  it("shows the total, the bar, the breakdown and the uploads", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage());
    renderPage();
    expect(await screen.findByText("4.0 GB")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "3.0 GB in photos, 1.0 GB in the trash" })).toBeInTheDocument();
    expect(screen.getByText("1,200 photos")).toBeInTheDocument();
    expect(screen.getByText(/300 RAW files/)).toBeInTheDocument();
    expect(screen.getByText(/in 1,500 files/)).toBeInTheDocument();
    expect(screen.getByText(/in 40 files/)).toBeInTheDocument();
    expect(screen.getByText("200 MB")).toBeInTheDocument();
    expect(screen.getByText("12 files")).toBeInTheDocument();
    expect(screen.getByText(/600 MB/)).toBeInTheDocument();
    expect(screen.getByText("900 files")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open the trash" })).toHaveAttribute("href", "/trash");
  });

  it("calls the thumbnail size a lower bound until everything is measured", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage({ derived_complete: false }));
    renderPage();
    expect(await screen.findByText("at least 200 MB")).toBeInTheDocument();
    expect(screen.getByText(/lower bound/)).toBeInTheDocument();
  });

  it("copes with an empty account", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(
      usage({
        originals_bytes: 0,
        trash_bytes: 0,
        stored_bytes: 0,
        derived_bytes: 0,
        photo_count: 0,
        raw_count: 0,
        file_count: 0,
        trash_count: 0,
        uploads: { count_24h: 0, bytes_24h: 0, count_30d: 0, bytes_30d: 0 },
      }),
    );
    renderPage();
    expect(await screen.findByText("0 photos")).toBeInTheDocument();
    expect(screen.queryByText(/RAW/)).not.toBeInTheDocument();
  });

  it("shows the error when it cannot load", async () => {
    vi.spyOn(api, "usage").mockRejectedValue(new ApiError(500, "internal error"));
    renderPage();
    expect(await screen.findByText("internal error")).toBeInTheDocument();
  });
});
