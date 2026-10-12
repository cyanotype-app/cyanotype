import { afterEach, describe, expect, it, vi } from "vitest";
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
  });
});
