import { describe, expect, it, vi } from "vitest";
import { ApiError, type CheckResponse } from "../api";
import { UploadQueue, type QueueDeps, type UploadItem } from "./queue";

const file = (name: string, size: number, type = "") => {
  const f = new File([new Uint8Array(size)], name, { type, lastModified: Date.UTC(2020, 0, 2, 3, 4, 5) });
  return f;
};
const onePart = vi.fn(
  async (): Promise<CheckResponse> => ({
    status: "upload",
    upload_id: "up1",
    part_size: 10,
    parts: [{ number: 1, url: "/p/1" }],
    uploaded: [],
  }),
);
const sha = "ab".repeat(32);

function setup(over: Partial<QueueDeps["api"]> = {}, deps: Partial<QueueDeps> = {}) {
  const base = {
    checkUpload: vi.fn(
      async (): Promise<CheckResponse> => ({
        status: "upload",
        upload_id: "up1",
        part_size: 10,
        parts: [
          { number: 1, url: "/p/1" },
          { number: 2, url: "/p/2" },
          { number: 3, url: "/p/3" },
        ],
        uploaded: [],
      }),
    ),
    putPart: vi.fn(async (url: string) => ({ etag: `"etag${url.slice(-1)}"` })),
    completeUpload: vi.fn(async () => ({ asset_id: "asset-1" })),
  };
  const api = { ...base, ...over };
  const q = new UploadQueue({ api, hash: async () => sha, delay: async () => {}, ...deps });
  return { api: base, q };
}
const settle = async (q: UploadQueue) => {
  await vi.waitFor(() => expect(q.active).toBe(false));
  return q.getSnapshot();
};

describe("UploadQueue", () => {
  it("hashes, checks with the proxy transport and metadata, uploads slices, completes", async () => {
    const { api, q } = setup();
    const added: UploadItem[] = [];
    q.onAssetAdded((i) => added.push(i));
    q.add([file("IMG.CR2", 25)]);
    const [it] = await settle(q);
    expect(it?.status).toBe("done");
    expect(it?.assetId).toBe("asset-1");
    expect(api.checkUpload).toHaveBeenCalledWith({
      sha256: sha,
      size_bytes: 25,
      filename: "IMG.CR2",
      content_type: "application/octet-stream",
      file_modified_at: "2020-01-02T03:04:05.000Z",
      transport: "proxy",
    });
    const sizes = api.putPart.mock.calls.map((c) => (c as unknown as [string, Blob])[1].size).sort((a, b) => a - b);
    expect(sizes).toEqual([5, 10, 10]);
    expect(api.completeUpload).toHaveBeenCalledWith("up1", [
      { number: 1, etag: '"etag1"' },
      { number: 2, etag: '"etag2"' },
      { number: 3, etag: '"etag3"' },
    ]);
    expect(added).toHaveLength(1);
  });

  it("marks existing content as a duplicate without uploading", async () => {
    const { api, q } = setup({ checkUpload: vi.fn(async () => ({ status: "exists" as const, asset_id: "a9" })) });
    const added = vi.fn();
    q.onAssetAdded(added);
    q.add([file("a.jpg", 5, "image/jpeg")]);
    expect((await settle(q))[0]?.status).toBe("duplicate");
    expect(api.putPart).not.toHaveBeenCalled();
    expect(added).not.toHaveBeenCalled();
  });

  it("resumes: only missing parts are sent, stored parts are passed to complete", async () => {
    const { api, q } = setup({
      checkUpload: vi.fn(async () => ({
        status: "upload" as const,
        upload_id: "up1",
        part_size: 10,
        parts: [{ number: 2, url: "/p/2" }],
        uploaded: [{ number: 1, etag: '"old"' }],
      })),
    });
    q.add([file("a.jpg", 15)]);
    expect((await settle(q))[0]?.status).toBe("done");
    expect(api.putPart).toHaveBeenCalledTimes(1);
    expect(api.completeUpload).toHaveBeenCalledWith("up1", [
      { number: 1, etag: '"old"' },
      { number: 2, etag: '"etag2"' },
    ]);
  });

  it("retries a part on network errors and 5xx, then succeeds", async () => {
    let n = 0;
    const putPart = vi.fn(async () => {
      if (++n === 1) throw new ApiError(0, "net");
      if (n === 2) throw new ApiError(502, "bad gateway");
      return { etag: '"ok"' };
    });
    const { q } = setup({ putPart, checkUpload: onePart }, {});
    q.add([file("a.jpg", 5)]);
    expect((await settle(q))[0]?.status).toBe("done");
    expect(putPart).toHaveBeenCalledTimes(3);
  });

  it("does not retry a 4xx, and reports the reason; retry() runs it again", async () => {
    const putPart = vi.fn().mockRejectedValueOnce(new ApiError(400, "part 1 must be exactly 5 bytes"));
    putPart.mockResolvedValue({ etag: '"ok"' });
    const { q } = setup({ putPart, checkUpload: onePart });
    q.add([file("a.jpg", 5)]);
    const [failed] = await settle(q);
    expect(failed?.status).toBe("failed");
    expect(failed?.error).toBe("part 1 must be exactly 5 bytes");
    expect(putPart).toHaveBeenCalledTimes(1);
    q.retry(failed!.id);
    expect((await settle(q))[0]?.status).toBe("done");
  });

  it("gives up after the attempt limit", async () => {
    const putPart = vi.fn().mockRejectedValue(new ApiError(503, "unavailable"));
    const { q } = setup({ putPart, checkUpload: onePart }, { maxAttempts: 3 });
    q.add([file("a.jpg", 5)]);
    expect((await settle(q))[0]?.status).toBe("failed");
    expect(putPart).toHaveBeenCalledTimes(3);
  });

  it("fails empty files without calling the server", async () => {
    const { api, q } = setup();
    q.add([file("empty.jpg", 0)]);
    const [it] = await settle(q);
    expect(it?.status).toBe("failed");
    expect(api.checkUpload).not.toHaveBeenCalled();
  });

  it("limits files in flight to 2 and parts in flight to 3", async () => {
    let hashing = 0, maxHashing = 0, parts = 0, maxParts = 0;
    const gate: (() => void)[] = [];
    const { q } = setup(
      {
        checkUpload: vi.fn(async () => ({
          status: "upload" as const,
          upload_id: "u",
          part_size: 1,
          parts: [1, 2, 3, 4, 5, 6].map((number) => ({ number, url: `/p/${number}` })),
          uploaded: [],
        })),
        putPart: vi.fn(async () => {
          maxParts = Math.max(maxParts, ++parts);
          await new Promise<void>((r) => gate.push(r));
          parts--;
          return { etag: '"e"' };
        }),
      },
      {
        hash: async () => {
          maxHashing = Math.max(maxHashing, ++hashing);
          await Promise.resolve();
          hashing--;
          return sha;
        },
      },
    );
    q.add([file("a.jpg", 6), file("b.jpg", 6), file("c.jpg", 6)]);
    const statuses = () => q.getSnapshot().map((i) => i.status);
    await vi.waitFor(() => expect(maxParts).toBe(3));
    expect(statuses().filter((s) => s === "queued")).toHaveLength(1); // third file waits for a slot
    while (q.active) {
      gate.splice(0).forEach((r) => r());
      await new Promise((r) => setTimeout(r, 0));
    }
    expect(maxParts).toBe(3);
    expect(maxHashing).toBeLessThanOrEqual(2);
    expect(q.getSnapshot().every((i) => i.status === "done")).toBe(true);
  });

  it("summarises progress and clears finished entries", async () => {
    const { q } = setup({ checkUpload: vi.fn(async () => ({ status: "exists" as const, asset_id: "a" })) });
    q.add([file("a.jpg", 5), file("b.jpg", 5)]);
    await settle(q);
    expect(q.summary()).toMatchObject({ total: 2, finished: 2, duplicate: 2, active: false, progress: 1 });
    q.clearFinished();
    expect(q.getSnapshot()).toHaveLength(0);
  });
});
