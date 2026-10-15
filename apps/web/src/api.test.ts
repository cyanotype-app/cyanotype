import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "./api";

function mockFetch(res: Response | Error) {
  const fn = vi.fn(async () => {
    if (res instanceof Error) throw res;
    return res.clone(); // a Response body can be read once; the test may call the API repeatedly
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

const jsonRes = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

afterEach(() => vi.unstubAllGlobals());

describe("api client", () => {
  it("sends the session cookie and parses JSON", async () => {
    const f = mockFetch(jsonRes(200, { id: "u1", email: "a@b.co" }));
    await expect(api.me()).resolves.toEqual({ id: "u1", email: "a@b.co" });
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/auth/me");
    expect(init.credentials).toBe("include");
  });

  it("posts credentials as JSON", async () => {
    const f = mockFetch(jsonRes(200, { id: "u1" }));
    await api.login("a@b.co", "pw");
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/auth/login");
    expect(init.method).toBe("POST");
    expect(init.body).toBe(JSON.stringify({ email: "a@b.co", password: "pw" }));
    expect(new Headers(init.headers).get("content-type")).toBe("application/json");
  });

  it("surfaces the server's {error} message with the status", async () => {
    mockFetch(jsonRes(403, { error: "registration is closed" }));
    const err = await api.register("a@b.co", "longenough").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({ status: 403, message: "registration is closed" });
  });

  it("falls back to a generic message for non-JSON error bodies", async () => {
    mockFetch(new Response("<html>Bad gateway</html>", { status: 502 }));
    await expect(api.listAssets()).rejects.toMatchObject({ status: 502, message: "Request failed (502)" });
  });

  it("reports network failures as status 0", async () => {
    mockFetch(new TypeError("Failed to fetch"));
    await expect(api.me()).rejects.toMatchObject({ status: 0 });
  });

  it("treats 204 as success with no body", async () => {
    mockFetch(new Response(null, { status: 204 }));
    await expect(api.logout()).resolves.toBeUndefined();
  });

  it("builds the list query with limit and an encoded before cursor", async () => {
    const f = mockFetch(jsonRes(200, { items: [], next_before: null }));
    await api.listAssets({ limit: 50, before: "2026-01-02T03:04:05.123456+00:00" });
    expect((f.mock.calls[0] as unknown as [string])[0]).toBe(
      "/assets?limit=50&before=2026-01-02T03%3A04%3A05.123456%2B00%3A00",
    );
    await api.listAssets();
    expect((f.mock.calls[1] as unknown as [string])[0]).toBe("/assets");
    // `cursor` wins over the deprecated `before`.
    await api.listAssets({ limit: 10, cursor: "6162|x", before: "2026-01-01T00:00:00Z" });
    expect((f.mock.calls[2] as unknown as [string])[0]).toBe("/assets?limit=10&cursor=6162%7Cx");
  });

  it("check defaults to the proxy transport and posts JSON", async () => {
    const f = mockFetch(jsonRes(200, { status: "exists", asset_id: "a1" }));
    await expect(
      api.checkUpload({ sha256: "00", size_bytes: 3, filename: "a.jpg", file_modified_at: "2020-01-01T00:00:00.000Z" }),
    ).resolves.toEqual({ status: "exists", asset_id: "a1" });
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/assets/check");
    expect(JSON.parse(init.body as string)).toMatchObject({ transport: "proxy", filename: "a.jpg", size_bytes: 3 });
  });

  it("completes an upload with the part list", async () => {
    const f = mockFetch(jsonRes(201, { asset_id: "a1" }));
    await api.completeUpload("up/1", [{ number: 1, etag: '"e"' }]);
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/assets/complete");
    expect(JSON.parse(init.body as string)).toEqual({ upload_id: "up/1", parts: [{ number: 1, etag: '"e"' }] });
  });
});

describe("trash api", () => {
  it("posts ids to trash / restore / purge and returns the affected count", async () => {
    const f = mockFetch(jsonRes(200, { count: 2 }));
    await expect(api.trashAssets(["a", "b"])).resolves.toEqual({ count: 2 });
    await api.restoreAssets(["a"]);
    await api.purgeAssets(["b"]);
    const calls = f.mock.calls as unknown as [string, RequestInit][];
    expect(calls.map(([u, i]) => [u, i.method, JSON.parse(i.body as string)])).toEqual([
      ["/assets/trash", "POST", { ids: ["a", "b"] }],
      ["/assets/restore", "POST", { ids: ["a"] }],
      ["/assets/purge", "POST", { ids: ["b"] }],
    ]);
  });

  it("splits more than 1000 ids into several requests and sums the counts", async () => {
    const f = mockFetch(jsonRes(200, { count: 1000 }));
    const ids = Array.from({ length: 2500 }, (_, i) => `id-${i}`);
    await expect(api.trashAssets(ids)).resolves.toEqual({ count: 3000 }); // the mock answers 1000 each time
    const sizes = (f.mock.calls as unknown as [string, RequestInit][]).map(([, i]) => JSON.parse(i.body as string).ids.length);
    expect(sizes).toEqual([1000, 1000, 500]);
    expect(f).toHaveBeenCalledTimes(3);
  });

  it("sends nothing for an empty list", async () => {
    const f = mockFetch(jsonRes(200, { count: 0 }));
    await expect(api.trashAssets([])).resolves.toEqual({ count: 0 });
    expect(f).not.toHaveBeenCalled();
  });

  it("empties the trash with a bodyless POST", async () => {
    const f = mockFetch(jsonRes(200, { count: 7 }));
    await expect(api.emptyTrash()).resolves.toEqual({ count: 7 });
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect([url, init.method, init.body]).toEqual(["/assets/trash/empty", "POST", undefined]);
  });

  it("lists the trash with limit and cursor", async () => {
    const f = mockFetch(jsonRes(200, { items: [], next_cursor: null, total: 0, retention_days: 30 }));
    await api.listTrash({ limit: 60, cursor: "ab|c" });
    await api.listTrash();
    const urls = (f.mock.calls as unknown as [string][]).map(([u]) => u);
    expect(urls).toEqual(["/assets/trash?limit=60&cursor=ab%7Cc", "/assets/trash"]);
  });

  it("surfaces server errors from bulk calls", async () => {
    mockFetch(jsonRes(400, { error: "at most 1000 ids per request" }));
    await expect(api.restoreAssets(["a"])).rejects.toMatchObject({ status: 400, message: "at most 1000 ids per request" });
  });
});

describe("putPart", () => {
  class FakeXhr {
    static last: FakeXhr;
    upload: { onprogress?: (e: { loaded: number }) => void } = {};
    headers: Record<string, string> = {};
    method = "";
    url = "";
    withCredentials = false;
    responseType = "";
    status = 0;
    responseText = "";
    body: unknown;
    onload?: () => void;
    onerror?: () => void;
    onabort?: () => void;
    ontimeout?: () => void;
    constructor() {
      FakeXhr.last = this;
    }
    open(m: string, u: string) {
      this.method = m;
      this.url = u;
    }
    setRequestHeader(k: string, v: string) {
      this.headers[k] = v;
    }
    send(b: unknown) {
      this.body = b;
    }
    abort() {
      this.onabort?.();
    }
    reply(status: number, text: string) {
      this.status = status;
      this.responseText = text;
      this.onload?.();
    }
  }
  beforeEach(() => vi.stubGlobal("XMLHttpRequest", FakeXhr));

  it("PUTs the blob with credentials, reports progress and resolves the etag", async () => {
    const blob = new Blob(["abc"]);
    const onProgress = vi.fn();
    const p = api.putPart("/assets/uploads/u/parts/1", blob, { onProgress });
    const x = FakeXhr.last;
    expect([x.method, x.url, x.withCredentials, x.body]).toEqual(["PUT", "/assets/uploads/u/parts/1", true, blob]);
    expect(x.headers["Content-Type"]).toBe("application/octet-stream");
    x.upload.onprogress?.({ loaded: 2 });
    expect(onProgress).toHaveBeenCalledWith(2);
    x.reply(200, JSON.stringify({ etag: '"abc"' }));
    await expect(p).resolves.toEqual({ etag: '"abc"' });
  });

  it("rejects with the server's error text and status", async () => {
    const p = api.putPart("/x", new Blob(["a"]));
    FakeXhr.last.reply(400, JSON.stringify({ error: "part 2 must be exactly 4 bytes" }));
    await expect(p).rejects.toMatchObject({ status: 400, message: "part 2 must be exactly 4 bytes" });
  });

  it("falls back to a generic message for non-JSON errors, and maps aborts and network errors to status 0", async () => {
    const p = api.putPart("/x", new Blob(["a"]));
    FakeXhr.last.reply(502, "<html>bad gateway</html>");
    await expect(p).rejects.toMatchObject({ status: 502, message: "Request failed (502)" });
    const p2 = api.putPart("/x", new Blob(["a"]));
    FakeXhr.last.onerror?.();
    await expect(p2).rejects.toMatchObject({ status: 0 });
    const ac = new AbortController();
    const p3 = api.putPart("/x", new Blob(["a"]), { signal: ac.signal });
    ac.abort();
    await expect(p3).rejects.toMatchObject({ status: 0, message: "Upload canceled." });
  });
});
