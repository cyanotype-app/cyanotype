import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type TakeoutPlan } from "../api";
import { UploadProvider } from "../upload/UploadProvider";
import { TakeoutImport } from "./TakeoutImport";

const plan: TakeoutPlan = {
  photos: [
    { path: "Takeout/Google Photos/Trip/a.jpg", sidecar: "Takeout/Google Photos/Trip/a.jpg.json", album: "Trip", edited: false },
    { path: "Takeout/Google Photos/Photos from 2019/b.jpg", sidecar: null, album: null, edited: false },
  ],
  videos: ["Takeout/Google Photos/Trip/v.mp4"],
  other: [],
  trashed: [],
  albums: { Trip: 1 },
  unused_sidecars: [],
};

function file(path: string, body = "x"): File {
  const f = new File([body], path.split("/").pop()!);
  Object.defineProperty(f, "webkitRelativePath", { value: path });
  return f;
}

function renderPage(hash = vi.fn(async () => "a".repeat(64))) {
  const checkUpload = vi.fn(async () => ({ status: "exists" as const, asset_id: "x" }));
  render(
    <MemoryRouter>
      <UploadProvider deps={{ api: { checkUpload, putPart: vi.fn(), completeUpload: vi.fn() }, hash, delay: async () => undefined }}>
        <TakeoutImport />
      </UploadProvider>
    </MemoryRouter>,
  );
  return { checkUpload };
}

afterEach(() => vi.restoreAllMocks());

describe("TakeoutImport", () => {
  it("previews the counts and options before anything is uploaded, then uploads with the metadata", async () => {
    vi.spyOn(api, "takeoutPlan").mockResolvedValue(plan);
    const { checkUpload } = renderPage();
    const user = userEvent.setup();
    await user.upload(screen.getByLabelText("Takeout folder"), [
      file("Takeout/Google Photos/Trip/a.jpg"),
      file("Takeout/Google Photos/Trip/a.jpg.json", '{"favorited":true}'),
      file("Takeout/Google Photos/Photos from 2019/b.jpg"),
      file("Takeout/Google Photos/Trip/v.mp4"),
    ]);
    const prev = await screen.findByRole("region", { name: "What will be imported" });
    expect(within(prev).getByText("2 photos to import")).toBeInTheDocument();
    expect(within(prev).getByText(/1 photo with their Google Photos details/)).toBeInTheDocument();
    expect(within(prev).getByText(/1 photo without a details file/)).toBeInTheDocument();
    expect(within(prev).getByText(/1 album from named folders/)).toBeInTheDocument();
    expect(within(prev).getByText("Trip")).toBeInTheDocument();
    expect(within(prev).getByText(/Skipped: 1 video/)).toBeInTheDocument();
    expect(checkUpload).not.toHaveBeenCalled();

    // Albums off, favorites left on.
    await user.click(screen.getByLabelText("Create albums from folders"));
    await user.click(screen.getByRole("button", { name: "Import 2 photos" }));
    expect(await screen.findByRole("status", { name: "Import started" })).toBeInTheDocument();
    await waitFor(() => expect(checkUpload).toHaveBeenCalledTimes(2));
    const sent = (checkUpload.mock.calls as unknown as [{ filename: string; takeout?: unknown }][]).map(([r]) => r);
    expect(sent.find((r) => r.filename === "a.jpg")!.takeout).toEqual({ sidecar: '{"favorited":true}', album: undefined, favorites: true });
    expect(sent.find((r) => r.filename === "b.jpg")!.takeout).toBeUndefined();
  });

  it("explains zip files instead of trying to read them", async () => {
    const ask = vi.spyOn(api, "takeoutPlan");
    renderPage();
    const user = userEvent.setup();
    await user.upload(screen.getByLabelText("Takeout folder"), [new File(["z"], "takeout-001.zip")]);
    expect(await screen.findByText(/Unzip them first/)).toBeInTheDocument();
    expect(ask).not.toHaveBeenCalled();
  });

  it("shows a failed read and lets the person try again", async () => {
    vi.spyOn(api, "takeoutPlan").mockRejectedValue(new Error("at most 100000 file names"));
    renderPage();
    const user = userEvent.setup();
    await user.upload(screen.getByLabelText("Takeout folder"), [file("T/a.jpg")]);
    expect(await screen.findByText(/at most 100000/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Choose the Takeout folder" })).toBeEnabled();
  });
});
