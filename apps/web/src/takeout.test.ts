import { describe, expect, it, vi } from "vitest";
import type { TakeoutPlan } from "./api";
import { batches, merge, planFiles, previewOf, relPath, uploadSet } from "./takeout";
import { UploadQueue } from "./upload/queue";

function file(path: string, body = "x"): File {
  const f = new File([body], path.split("/").pop()!, { type: "" });
  Object.defineProperty(f, "webkitRelativePath", { value: path });
  return f;
}

const plan = (over: Partial<TakeoutPlan> = {}): TakeoutPlan => ({
  photos: [],
  videos: [],
  other: [],
  trashed: [],
  albums: {},
  unused_sidecars: [],
  ...over,
});

describe("batches", () => {
  it("never splits a folder across requests", () => {
    const paths = ["a/1.jpg", "a/1.jpg.json", "b/2.jpg", "b/2.jpg.json", "c/3.jpg"];
    const out = batches(paths, 4);
    expect(out).toEqual([["a/1.jpg", "a/1.jpg.json", "b/2.jpg", "b/2.jpg.json"], ["c/3.jpg"]]);
    expect(batches([], 4)).toEqual([]);
  });

  it("cuts one huge folder in name order", () => {
    const paths = ["d/b.jpg", "d/a.jpg", "d/a.jpg.json", "d/b.jpg.json"];
    const out = batches(paths, 2);
    expect(out).toEqual([["d/a.jpg", "d/a.jpg.json"], ["d/b.jpg", "d/b.jpg.json"]]);
  });
});

describe("planFiles", () => {
  it("sends names only, without hidden or system files, and merges the answers", async () => {
    const ask = vi.fn(async (paths: string[]) =>
      plan({ photos: paths.map((p) => ({ path: p, sidecar: null, album: "Trip", edited: false })), albums: { Trip: paths.length } }),
    );
    const files = [file("T/Trip/a.jpg"), file("T/Trip/.DS_Store"), file("T/__MACOSX/Trip/._a.jpg"), file("T/Other/b.jpg")];
    const p = await planFiles(files, ask);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(ask.mock.calls[0]![0]).toEqual(["T/Trip/a.jpg", "T/Other/b.jpg"]);
    expect(p.photos).toHaveLength(2);
  });

  it("adds albums across requests", () => {
    const m = merge([plan({ albums: { A: 1, B: 2 } }), plan({ albums: { A: 3 }, videos: ["v.mp4"] })]);
    expect(m.albums).toEqual({ A: 4, B: 2 });
    expect(m.videos).toEqual(["v.mp4"]);
  });
});

describe("previewOf", () => {
  it("counts what the person is told before uploading", () => {
    const p = plan({
      photos: [
        { path: "T/a.jpg", sidecar: "T/a.jpg.json", album: null, edited: false },
        { path: "T/a-edited.jpg", sidecar: "T/a.jpg.json", album: null, edited: true },
        { path: "T/Z/b.jpg", sidecar: null, album: "Zoo", edited: false },
      ],
      videos: ["T/v.mp4", "T/w.mov"],
      other: ["T/x.gif"],
      trashed: [],
      albums: { Zoo: 1, Apes: 4 },
    });
    expect(previewOf(p)).toEqual({
      photos: 3,
      videos: 2,
      other: 1,
      trashed: 0,
      albums: [
        { name: "Apes", photos: 4 },
        { name: "Zoo", photos: 1 },
      ],
      withoutSidecar: 1,
      withSidecar: 2,
      edited: 1,
    });
  });
});

describe("uploadSet", () => {
  const photos = [
    { path: "T/Trip/a.jpg", sidecar: "T/Trip/a.jpg.json", album: "Trip", edited: false },
    { path: "T/Trip/b.jpg", sidecar: null, album: "Trip", edited: false },
    { path: "T/c.jpg", sidecar: null, album: null, edited: false },
    { path: "T/d.heic.txt", sidecar: null, album: null, edited: false },
  ];
  const files = [
    file("T/Trip/a.jpg"),
    file("T/Trip/a.jpg.json", '{"description":"hi"}'),
    file("T/Trip/b.jpg"),
    file("T/c.jpg"),
    file("T/d.heic.txt"),
  ];

  it("uploads only the photos, and builds each one's metadata when it is reached", async () => {
    const set = uploadSet(plan({ photos }), files, { favorites: true, albums: true });
    expect(set.files.map(relPath)).toEqual(["T/Trip/a.jpg", "T/Trip/b.jpg", "T/c.jpg"]);
    expect(await set.takeoutOf(set.files[0]!)).toEqual({ sidecar: '{"description":"hi"}', album: "Trip", favorites: true });
    expect(await set.takeoutOf(set.files[1]!)).toEqual({ sidecar: undefined, album: "Trip", favorites: true });
    expect(await set.takeoutOf(set.files[2]!)).toBeUndefined();
  });

  it("the options leave albums out and carry the favorites choice", async () => {
    const set = uploadSet(plan({ photos }), files, { favorites: false, albums: false });
    expect(await set.takeoutOf(set.files[0]!)).toEqual({ sidecar: '{"description":"hi"}', album: undefined, favorites: false });
    expect(await set.takeoutOf(set.files[1]!)).toBeUndefined();
  });

  it("does not send a sidecar over 64 KB", async () => {
    const big = file("T/Trip/a.jpg.json", "x".repeat(70_000));
    const set = uploadSet(plan({ photos: [photos[0]!] }), [file("T/Trip/a.jpg"), big], { favorites: true, albums: false });
    expect(await set.takeoutOf(set.files[0]!)).toBeUndefined();
  });
});

describe("the queue carries the metadata with the check call", () => {
  it("sends takeout with a Takeout file and nothing with an ordinary one, and a duplicate still counts as done", async () => {
    const checkUpload = vi.fn(async () => ({ status: "exists" as const, asset_id: "a1" }));
    const q = new UploadQueue({
      api: { checkUpload, putPart: vi.fn(), completeUpload: vi.fn() },
      hash: async (f) => (f.name === "a.jpg" ? "a".repeat(64) : "b".repeat(64)),
      delay: async () => undefined,
    });
    const a = file("T/a.jpg");
    const b = file("T/b.jpg");
    q.add([a], async () => ({ sidecar: '{"favorited":true}', album: "Trip", favorites: true }));
    q.add([b]);
    await vi.waitFor(() => expect(q.summary().finished).toBe(2));
    const calls = checkUpload.mock.calls as unknown as [Record<string, unknown>][];
    const sent = Object.fromEntries(calls.map(([r]) => [r.filename as string, r]));
    expect(sent["a.jpg"]!.takeout).toEqual({ sidecar: '{"favorited":true}', album: "Trip", favorites: true });
    expect("takeout" in sent["b.jpg"]!).toBe(false);
    expect(q.summary().duplicate).toBe(2);
  });
});
