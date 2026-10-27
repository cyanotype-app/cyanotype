import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, downloadHref, MAX_BULK_IDS, searchQuery } from "./api";

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

  it("sends a scope as one request, with the except list, and passes the batch back", async () => {
    const f = mockFetch(jsonRes(200, { count: 48_213, batch: "2026-10-16T10:00:00Z" }));
    await expect(api.trashAssets({ scope: "library", except: ["a"] })).resolves.toEqual({
      count: 48_213,
      batch: "2026-10-16T10:00:00Z",
    });
    await api.restoreAssets({ scope: { trash_batch: "2026-10-16T10:00:00Z" }, except: [] });
    await api.purgeAssets({ scope: "trash", except: [] });
    await api.favoriteAssets({ scope: "favorites", except: ["b"] }, false);
    await api.addToAlbum("al 1", { scope: "library", except: [] });
    await api.removeFromAlbum("al 1", { scope: { album: "al 1" }, except: ["c"] });
    const calls = f.mock.calls as unknown as [string, RequestInit][];
    expect(calls.map(([u, i]) => [u, JSON.parse(i.body as string)])).toEqual([
      ["/assets/trash", { scope: "library", except: ["a"] }],
      ["/assets/restore", { scope: { trash_batch: "2026-10-16T10:00:00Z" }, except: [] }],
      ["/assets/purge", { scope: "trash", except: [] }],
      ["/assets/favorite", { scope: "favorites", except: ["b"], favorite: false }],
      ["/assets/albums/al%201/assets", { scope: "library", except: [] }],
      ["/assets/albums/al%201/assets/remove", { scope: { album: "al 1" }, except: ["c"] }],
    ]);
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

describe("album calls", () => {
  const call = (f: ReturnType<typeof mockFetch>, n = 0) => {
    const [url, init] = f.mock.calls[n] as unknown as [string, RequestInit];
    return { url, method: init.method ?? "GET", body: init.body ? (JSON.parse(String(init.body)) as unknown) : undefined };
  };

  it("lists and reads albums under /assets/albums", async () => {
    const f = mockFetch(jsonRes(200, { items: [] }));
    await api.listAlbums();
    await api.getAlbum("a/b");
    expect(call(f, 0)).toMatchObject({ url: "/assets/albums", method: "GET" });
    expect(call(f, 1).url).toBe("/assets/albums/a%2Fb");
  });

  it("creates an album with its first photos, and patches with explicit nulls", async () => {
    const f = mockFetch(jsonRes(201, { id: "a1" }));
    await api.createAlbum({ title: "Trip", asset_ids: ["x", "y"] });
    expect(call(f)).toEqual({
      url: "/assets/albums",
      method: "POST",
      body: { title: "Trip", asset_ids: ["x", "y"] },
    });
    await api.updateAlbum("a1", { description: null, cover_asset_id: "x" });
    expect(call(f, 1)).toEqual({
      url: "/assets/albums/a1",
      method: "PATCH",
      body: { description: null, cover_asset_id: "x" },
    });
  });

  it("adds the photos beyond the server's cap to a new album in further requests", async () => {
    const ids = Array.from({ length: MAX_BULK_IDS + 5 }, (_, i) => `id-${i}`);
    const f = mockFetch(jsonRes(200, { id: "a1", count: 5 }));
    await api.createAlbum({ title: "Big", asset_ids: ids });
    expect(f).toHaveBeenCalledTimes(3); // create, add the rest, re-read
    expect((call(f, 0).body as { asset_ids: string[] }).asset_ids).toHaveLength(MAX_BULK_IDS);
    expect(call(f, 1)).toMatchObject({ url: "/assets/albums/a1/assets", method: "POST" });
    expect((call(f, 1).body as { ids: string[] }).ids).toEqual(ids.slice(MAX_BULK_IDS));
    expect(call(f, 2)).toMatchObject({ url: "/assets/albums/a1", method: "GET" });
  });

  it("adds, removes and deletes", async () => {
    const f = mockFetch(jsonRes(200, { count: 2 }));
    await expect(api.addToAlbum("a1", ["x", "y"])).resolves.toEqual({ count: 2 });
    await expect(api.removeFromAlbum("a1", ["x"])).resolves.toEqual({ count: 2 });
    expect(call(f, 0)).toEqual({ url: "/assets/albums/a1/assets", method: "POST", body: { ids: ["x", "y"] } });
    expect(call(f, 1)).toEqual({ url: "/assets/albums/a1/assets/remove", method: "POST", body: { ids: ["x"] } });
    mockFetch(new Response(null, { status: 204 }));
    await expect(api.deleteAlbum("a1")).resolves.toBeUndefined();
  });

  it("pages an album's photos with the cursor", async () => {
    const f = mockFetch(jsonRes(200, { items: [], next_cursor: null }));
    await api.listAlbumAssets("a1", { cursor: "c0ffee", limit: 60 });
    expect(call(f).url).toBe("/assets/albums/a1/assets?limit=60&cursor=c0ffee");
  });

  it("unpairs with POST /assets/{id}/unpair", async () => {
    const f = mockFetch(new Response(null, { status: 204 }));
    await expect(api.unpair("a b")).resolves.toBeUndefined();
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/assets/a%20b/unpair");
    expect(init.method).toBe("POST");
  });
});

const mockFetchJson = (body: unknown) => mockFetch(jsonRes(200, body));

describe("timeline, favorites and downloads", () => {
  it("reads the per-day counts, optionally for favorites only", async () => {
    const f = mockFetchJson({ total: 3, days: [{ day: "2026-10-14", count: 3 }] });
    expect(await api.timeline()).toEqual({ total: 3, days: [{ day: "2026-10-14", count: 3 }] });
    expect(f).toHaveBeenLastCalledWith("/assets/timeline", expect.anything());
    await api.timeline({ favorite: true });
    expect(f).toHaveBeenLastCalledWith("/assets/timeline?favorite=true", expect.anything());
  });

  it("lists with a `before` position and the favorite filter", async () => {
    const f = mockFetchJson({ items: [], next_cursor: null, next_before: null });
    await api.listAssets({ before: "2026-10-15T00:00:00.000Z", limit: 120, favorite: true });
    expect(f).toHaveBeenLastCalledWith("/assets?limit=120&favorite=true&before=2026-10-15T00%3A00%3A00.000Z", expect.anything());
    await api.listAssets({ cursor: "abc", before: "ignored" });
    expect(f).toHaveBeenLastCalledWith("/assets?cursor=abc", expect.anything());
  });

  it("favorites in chunks of at most 1000 ids and adds up what changed", async () => {
    const f = mockFetchJson({ count: 2 });
    const ids = Array.from({ length: 1500 }, (_, i) => `id-${i}`);
    expect(await api.favoriteAssets(ids, true)).toEqual({ count: 4 });
    expect(f).toHaveBeenCalledTimes(2);
    const [path, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/assets/favorite");
    expect(JSON.parse(init.body as string)).toMatchObject({ favorite: true, ids: ids.slice(0, 1000) });
    expect(JSON.parse((f.mock.calls[1] as unknown as [string, RequestInit])[1].body as string).ids).toHaveLength(500);
  });

  it("reads a photo's details from its own endpoint, and sends the lens filter", async () => {
    const f = mockFetchJson({ id: "a/b" });
    await api.getDetails("a/b");
    expect(f).toHaveBeenLastCalledWith("/assets/a%2Fb/details", expect.anything());
    expect(searchQuery({ camera: "Canon EOS R5", lens: "RF 24-70mm F2.8 L" })).toBe("camera=Canon+EOS+R5&lens=RF+24-70mm+F2.8+L");
    expect(searchQuery({ lens: "X", similar: "id" })).toBe("lens=X");
  });

  it("builds the search query from the fields that are set, and pages with the cursor", async () => {
    expect(searchQuery({})).toBe("");
    expect(searchQuery({ favorite: true, rating: 4 })).toBe("favorite=true&rating=4");
    expect(searchQuery({ q: "dog on the beach", favorite: false, camera: "", has_location: false })).toBe(
      "q=dog+on+the+beach&has_location=false",
    );
    const f = mockFetchJson({ items: [], next_cursor: null, ranked: true });
    await api.search({ q: "cat", from: "2024-01-01", type: "raw", favorite: true }, { cursor: "c1", limit: 60 });
    expect(f).toHaveBeenLastCalledWith("/assets/search?q=cat&from=2024-01-01&favorite=true&type=raw&limit=60&cursor=c1", expect.anything());
    // The weaker tier and what not to read out of the query.
    await api.search({ q: "dog in Nice", skip: "place" }, { cursor: "c2", weaker: true });
    expect(f).toHaveBeenLastCalledWith("/assets/search?q=dog+in+Nice&skip=place&cursor=c2&weaker=true", expect.anything());
    await api.search({ q: "dog" }, { weaker: false });
    expect(f).toHaveBeenLastCalledWith("/assets/search?q=dog", expect.anything());
    await api.searchFacets();
    expect(f).toHaveBeenLastCalledWith("/assets/search/facets", expect.anything());
  });

  it("asks for photos like one photo on its own path, with the filters and without text", async () => {
    const f = mockFetchJson({ items: [], next_cursor: null, ranked: true });
    const id = "6f1c2b0e-0000-4000-8000-000000000001";
    await api.search({ similar: id, q: "ignored", skip: "date", favorite: true, from: "2024-01-01" }, { cursor: "c1", limit: 60, weaker: true });
    expect(f).toHaveBeenLastCalledWith(
      `/assets/${id}/similar?from=2024-01-01&favorite=true&limit=60&cursor=c1&weaker=true`,
      expect.anything(),
    );
    expect(searchQuery({ similar: id })).toBe("");
  });

  it("sends a filter-only select-all as a search scope", async () => {
    const f = mockFetchJson({ count: 5 });
    await api.favoriteAssets({ scope: { search: { camera: "Canon EOS R5" } }, except: ["x"] }, true);
    const [, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(JSON.parse(init.body as string)).toEqual({ scope: { search: { camera: "Canon EOS R5" } }, except: ["x"], favorite: true });
  });

  it("points downloads at the endpoint that redirects to a presigned attachment URL", () => {
    expect(downloadHref("abc")).toBe("/assets/abc/download");
    expect(downloadHref("a b", "raw/1")).toBe("/assets/a%20b/download?file=raw%2F1");
  });
});

describe("the edited download", () => {
  it("points at the route and tells pending from ready without following the redirect", async () => {
    const { editedHref } = await import("./api");
    expect(editedHref("a b")).toBe("/assets/a%20b/download/edited");
    const f = vi.fn().mockResolvedValueOnce(jsonRes(202, { pending: true }));
    vi.stubGlobal("fetch", f);
    await expect(api.editedStatus("p1")).resolves.toBe("pending");
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/assets/p1/download/edited");
    expect(init.redirect).toBe("manual");
    // The browser reports a redirect it was told not to follow as an opaque one.
    f.mockResolvedValueOnce({ status: 0, type: "opaqueredirect" });
    await expect(api.editedStatus("p1")).resolves.toBe("ready");
  });

  it("surfaces the server's reason when the photo has nothing to apply", async () => {
    mockFetch(jsonRes(400, { error: "this photo has no rotate or crop to apply" }));
    const err = await api.editedStatus("p1").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(400);
    expect((err as ApiError).message).toContain("no rotate or crop");
  });

  it("sends a recipe, a relative turn or a revert with the other edits", async () => {
    const f = mockFetch(jsonRes(200, { updated: 1, unchanged: 0, missing: 0, failed: [] }));
    await api.editAssets(["a"], { recipe: { turn: 90 } });
    expect((f.mock.calls[0] as unknown as [string, RequestInit])[1].body).toBe(JSON.stringify({ ids: ["a"], recipe: { turn: 90 } }));
  });
});

describe("people in lists, scopes and searches", () => {
  it("lists and counts one person's photos through the person's own endpoints", async () => {
    const f = mockFetchJson({ total: 1, days: [{ day: "2026-10-14", count: 1 }] });
    await api.timeline({ person: "p 1" });
    expect(f).toHaveBeenLastCalledWith("/assets/people/p%201/timeline", expect.anything());
    await api.listAssets({ person: "p1", before: "2026-10-15T00:00:00.000Z", limit: 120 });
    expect(f).toHaveBeenLastCalledWith("/assets/people/p1/photos?limit=120&before=2026-10-15T00%3A00%3A00.000Z", expect.anything());
    await api.listAssets({ person: "p1", cursor: "6162|x" });
    expect(f).toHaveBeenLastCalledWith("/assets/people/p1/photos?cursor=6162%7Cx", expect.anything());
  });

  it("sends a person scope to a bulk endpoint, and the people filter in search", async () => {
    const f = mockFetchJson({ count: 4 });
    await api.favoriteAssets({ scope: { person: "p1" }, except: ["x"] }, true);
    const [url, init] = f.mock.calls.at(-1)! as unknown as [string, RequestInit];
    expect(url).toBe("/assets/favorite");
    expect(JSON.parse(init.body as string)).toEqual({ scope: { person: "p1" }, except: ["x"], favorite: true });
    await api.search({ people: "a,b", place: "Lisbon" });
    expect(f).toHaveBeenLastCalledWith("/assets/search?place=Lisbon&people=a%2Cb", expect.anything());
    await api.search({ similar: "s1", people: "a" });
    expect(f).toHaveBeenLastCalledWith("/assets/s1/similar?people=a", expect.anything());
  });

  it("lists and counts the photos of a kind of pet through the Pets endpoints, and sends its scope and filter", async () => {
    const f = mockFetchJson({ total: 1, days: [{ day: "2026-10-14", count: 1 }] });
    await api.timeline({ pets: "dog" });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/timeline?kind=dog", expect.anything());
    await api.listAssets({ pets: "cat", before: "2026-10-15T00:00:00.000Z", limit: 120 });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/photos?limit=120&kind=cat&before=2026-10-15T00%3A00%3A00.000Z", expect.anything());
    await api.listAssets({ pets: "any", cursor: "6162|x" });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/photos?kind=any&cursor=6162%7Cx", expect.anything());
    await api.petsStatus();
    expect(f).toHaveBeenLastCalledWith("/assets/pets/status", expect.anything());
    await api.pets();
    expect(f).toHaveBeenLastCalledWith("/assets/pets", expect.anything());
    await api.favoriteAssets({ scope: { pets: "dog" }, except: [] }, true);
    const [, init] = f.mock.calls.at(-1)! as unknown as [string, RequestInit];
    expect(JSON.parse(init.body as string)).toEqual({ scope: { pets: "dog" }, except: [], favorite: true });
    await api.search({ pets: "cat", place: "Lisbon", skip: "pets" });
    expect(f).toHaveBeenLastCalledWith("/assets/search?place=Lisbon&pets=cat&skip=pets", expect.anything());
  });

  it("reads and changes named pets through their own endpoints", async () => {
    const f = mockFetchJson({});
    const last = () => f.mock.calls.at(-1)! as unknown as [string, RequestInit];
    const body = () => JSON.parse(last()[1].body as string) as unknown;
    const method = () => last()[1].method;
    await api.namedPets();
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named", expect.anything());
    await api.namedPet("p 1");
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p%201", expect.anything());
    await api.createPet({ name: "Pet A", kind: "dog", asset_ids: ["a1"] });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named", expect.anything());
    expect(body()).toEqual({ name: "Pet A", kind: "dog", asset_ids: ["a1"] });
    await api.updatePet("p1", { name: "Pet B", cover_detection_id: "d1" });
    expect(method()).toBe("PATCH");
    expect(body()).toEqual({ name: "Pet B", cover_detection_id: "d1" });
    await api.deletePet("p1");
    expect(method()).toBe("DELETE");
    await api.mergePet("p1", "p2");
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/merge", expect.anything());
    expect(body()).toEqual({ into: "p2" });
    await api.petDetections("p1", { limit: 24, cursor: "c" });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/detections?limit=24&cursor=c", expect.anything());
    await api.addToPet("p1", { detection_ids: ["d1"] });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/photos", expect.anything());
    expect(body()).toEqual({ detection_ids: ["d1"] });
    await api.removeFromPet("p1", { asset_ids: ["a1"] });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/photos/remove", expect.anything());
    await api.petSuggestions("p1", { tier: "maybe", limit: 60 });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/suggestions?limit=60&tier=maybe", expect.anything());
    await api.answerPetSuggestions("p1", { confirm: ["d1"], reject: ["d2"] });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/suggestions/answer", expect.anything());
    expect(body()).toEqual({ confirm: ["d1"], reject: ["d2"] });
    await api.assignPetDetection("d1", "p1");
    expect(f).toHaveBeenLastCalledWith("/assets/pets/detections/d1/assign", expect.anything());
    expect(body()).toEqual({ pet_id: "p1" });
    await api.rejectPetDetection("d1", "p1");
    expect(f).toHaveBeenLastCalledWith("/assets/pets/detections/d1/reject", expect.anything());
    await api.assetPets("a1");
    expect(f).toHaveBeenLastCalledWith("/assets/a1/pets", expect.anything());
  });

  it("lists and counts the photos of a named pet, and sends its scope and filter", async () => {
    const f = mockFetchJson({ total: 1, days: [{ day: "2026-10-14", count: 1 }] });
    await api.timeline({ pet: "p1" });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/timeline", expect.anything());
    await api.listAssets({ pet: "p1", cursor: "6162|x", limit: 120 });
    expect(f).toHaveBeenLastCalledWith("/assets/pets/named/p1/photos?limit=120&cursor=6162%7Cx", expect.anything());
    await api.trashAssets({ scope: { pet: "p1" }, except: ["a1"] });
    const [, init] = f.mock.calls.at(-1)! as unknown as [string, RequestInit];
    expect(JSON.parse(init.body as string)).toEqual({ scope: { pet: "p1" }, except: ["a1"] });
    await api.search({ pet: "p1,p2", place: "Lisbon", skip: "pet" });
    expect(f).toHaveBeenLastCalledWith("/assets/search?place=Lisbon&pet=p1%2Cp2&skip=pet", expect.anything());
    await api.search({ similar: "s1", pet: "p1" });
    expect(f).toHaveBeenLastCalledWith("/assets/s1/similar?pet=p1", expect.anything());
  });

  it("asks for trips by the user's today, a page size and the day to page from", async () => {
    const f = mockFetch(jsonRes(200, { enabled: true, trips: [], next_before: null }));
    await api.trips({ limit: 12, before: "2026-09-01", date: "2026-10-26" });
    expect(f).toHaveBeenLastCalledWith("/assets/trips?date=2026-10-26&limit=12&before=2026-09-01", expect.anything());
  });

  it("lists a trip's photos by their dates, without the ones taken out of memories", async () => {
    const f = mockFetch(jsonRes(200, { items: [], next_cursor: null, next_before: null }));
    await api.listAssets({ from: "2026-10-12", to: "2026-10-19", memories: true, limit: 60 });
    expect(f).toHaveBeenLastCalledWith("/assets?from=2026-10-12&to=2026-10-19&memories=true&limit=60", expect.anything());
  });
});
