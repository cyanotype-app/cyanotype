import { describe, expect, it } from "vitest";
import { searchQuery, type Processing } from "./api";
import {
  RECENT_MAX,
  addSkip,
  chipsOf,
  clearRecent,
  dateRangeLabel,
  dateShortcuts,
  quickFilters,
  hasFilters,
  indexingNote,
  indexingPercent,
  similarUrl,
  isEmptySearch,
  loadRecent,
  paramsFromUrl,
  parseSkip,
  pushRecent,
  reindexNote,
  suggestionsFor,
  urlFromParams,
  yearRange,
} from "./searchText";

const memory = () => {
  const m = new Map<string, string>();
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
  };
};

describe("rating filter", () => {
  it("reads 1 to 5 from the URL, drops the rest, and labels the chip", () => {
    expect(paramsFromUrl(new URLSearchParams("rating=3")).rating).toBe(3);
    for (const bad of ["0", "6", "x", "2.5", ""]) expect(paramsFromUrl(new URLSearchParams(`rating=${bad}`)).rating).toBeUndefined();
    expect(chipsOf({ rating: 4 })).toEqual([{ clears: ["rating"], label: "4 stars & up" }]);
    expect(chipsOf({ rating: 5 })).toEqual([{ clears: ["rating"], label: "5 stars" }]);
    expect(hasFilters({ rating: 1 })).toBe(true);
  });
});

describe("search URL", () => {
  it("round-trips every filter", () => {
    const p = {
      q: "dog on the beach",
      from: "2024-01-01",
      to: "2024-12-31",
      favorite: true,
      rating: 4,
      camera: "Canon EOS R5",
      place: "Lisbon",
      type: "raw" as const,
      album: "6f1c2d3e-0000-4000-8000-000000000001",
      has_location: false,
    };
    expect(paramsFromUrl(new URLSearchParams(urlFromParams(p)))).toEqual(p);
  });

  it("round-trips the people filter, dropping bad ids", () => {
    const a = "6f1c2b0e-0000-4000-8000-000000000001";
    const b = "6f1c2b0e-0000-4000-8000-000000000002";
    expect(paramsFromUrl(new URLSearchParams({ people: `${a},${b}` }))).toEqual({ people: `${a},${b}` });
    expect(paramsFromUrl(new URLSearchParams({ people: `nope,${a},${a}` }))).toEqual({ people: a });
    expect(paramsFromUrl(new URLSearchParams({ people: "nope" }))).toEqual({});
    expect(urlFromParams({ people: `${a},${b}`, place: "Lisbon" })).toBe(`place=Lisbon&people=${encodeURIComponent(`${a},${b}`)}`);
    expect(hasFilters({ people: a })).toBe(true);
  });

  it("drops what the server would reject instead of sending it", () => {
    const p = paramsFromUrl(
      new URLSearchParams("q=%20%20&from=yesterday&to=2024-5-1&type=gif&album=nope&favorite=1&has_location=maybe&camera=%20"),
    );
    expect(p).toEqual({});
    expect(isEmptySearch(p)).toBe(true);
  });

  it("caps the query and tells a query from a filter", () => {
    expect(paramsFromUrl(new URLSearchParams({ q: "x".repeat(500) })).q).toHaveLength(200);
    expect(hasFilters({ q: "cat" })).toBe(false);
    expect(hasFilters({ q: "cat", favorite: true })).toBe(true);
    expect(urlFromParams({ has_location: true })).toBe("has_location=true");
    expect(urlFromParams({})).toBe("");
  });
});

describe("the named pet filter", () => {
  const A = "11111111-1111-4111-8111-111111111111";
  const B = "22222222-2222-4222-8222-222222222222";
  it("reads valid ids from the address, once each, and writes them back", () => {
    expect(paramsFromUrl(new URLSearchParams({ pet: `${A},${A},nope,${B}` }))).toEqual({ pet: `${A},${B}` });
    expect(paramsFromUrl(new URLSearchParams({ pet: "nope" }))).toEqual({});
    expect(urlFromParams({ pet: A, place: "Lisbon" })).toBe(`place=Lisbon&pet=${A}`);
    expect(hasFilters({ pet: A })).toBe(true);
    expect(searchQuery({ pet: `${A},${B}`, favorite: true })).toBe(`favorite=true&pet=${encodeURIComponent(`${A},${B}`)}`);
  });
  it("is a chip with the pet's name, and gives the words back with skip=pet", () => {
    expect(chipsOf({ pet: A }, undefined, [], [{ id: A, name: "Pet A" }])).toEqual([{ clears: ["pet"], label: "Pet A" }]);
    expect(chipsOf({ pet: `${A},${B}` }, undefined, [], [{ id: A, name: "Pet A" }])[0]!.label).toBe("Pet A and Unknown pet");
    expect(parseSkip("pet,pets,x")).toBe("pets,pet");
    expect(addSkip("date", "pet")).toBe("date,pet");
  });
});

describe("the pets filter", () => {
  it("round-trips a kind and drops anything else", () => {
    expect(paramsFromUrl(new URLSearchParams({ pets: "dog" }))).toEqual({ pets: "dog" });
    expect(paramsFromUrl(new URLSearchParams({ pets: "any", place: "Lisbon" }))).toEqual({ place: "Lisbon", pets: "any" });
    expect(paramsFromUrl(new URLSearchParams({ pets: "bird" }))).toEqual({});
    expect(urlFromParams({ pets: "cat", place: "Lisbon" })).toBe("place=Lisbon&pets=cat");
    expect(hasFilters({ pets: "any" })).toBe(true);
    expect(searchQuery({ pets: "dog", favorite: true })).toBe("favorite=true&pets=dog");
  });

  it("is one chip that clears it, named for the kind", () => {
    expect(chipsOf({ pets: "dog" })).toEqual([{ clears: ["pets"], label: "Dogs" }]);
    expect(chipsOf({ pets: "cat" })[0]!.label).toBe("Cats");
    expect(chipsOf({ pets: "any" })[0]!.label).toBe("Pets");
  });

  it("is one of the things the server may be told not to read out of the words", () => {
    expect(parseSkip("pets, DATE")).toBe("date,pets");
    expect(addSkip("people", "pets")).toBe("people,pets");
  });
});

describe("what the server may not read out of the query (skip)", () => {
  it("keeps only date, place and people, once each, in a fixed order", () => {
    expect(parseSkip("place,date")).toBe("date,place");
    expect(parseSkip("people, PLACE")).toBe("place,people");
    expect(addSkip("date", "people")).toBe("date,people");
    expect(parseSkip("DATE, date,nonsense")).toBe("date");
    expect(parseSkip("nonsense")).toBeUndefined();
    expect(parseSkip(null)).toBeUndefined();
    expect(addSkip(undefined, "place")).toBe("place");
    expect(addSkip("place", "date")).toBe("date,place");
    expect(addSkip("date", "date")).toBe("date");
  });

  it("round-trips through the URL and is not a filter", () => {
    const p = paramsFromUrl(new URLSearchParams("q=dog+in+Nice&skip=place,date,x"));
    expect(p).toEqual({ q: "dog in Nice", skip: "date,place" });
    expect(urlFromParams(p)).toBe("q=dog+in+Nice&skip=date%2Cplace");
    expect(hasFilters(p)).toBe(false);
    expect(chipsOf(p)).toEqual([]);
  });
});

describe("More like this URL", () => {
  const id = "6f1c2b0e-0000-4000-8000-000000000001";

  it("reads the source photo, drops the text next to it, and writes the same URL back", () => {
    const p = paramsFromUrl(new URLSearchParams(`similar=${id}&q=dog&skip=date&favorite=true`));
    expect(p).toEqual({ similar: id, favorite: true });
    expect(urlFromParams(p)).toBe(`favorite=true&similar=${id}`);
    expect(similarUrl(id)).toBe(`/search?similar=${id}`);
    expect(paramsFromUrl(new URLSearchParams("similar=not-a-photo"))).toEqual({});
  });

  it("is a search of its own but not a filter", () => {
    expect(isEmptySearch({ similar: id })).toBe(false);
    expect(hasFilters({ similar: id })).toBe(false);
    expect(hasFilters({ similar: id, camera: "X" })).toBe(true);
  });
});

describe("filter chips", () => {
  it("labels a whole year, a day, a span and open ends", () => {
    expect(dateRangeLabel("2024-01-01", "2024-12-31")).toBe("2024");
    expect(dateRangeLabel("2024-03-01", "2024-03-01")).toBe("Mar 1, 2024");
    expect(dateRangeLabel("2024-03-01", "2024-03-09")).toBe("Mar 1, 2024 to Mar 9, 2024");
    expect(dateRangeLabel("2024-03-01")).toBe("From Mar 1, 2024");
    expect(dateRangeLabel(undefined, "2024-03-01")).toBe("Until Mar 1, 2024");
    expect(yearRange("2021")).toEqual({ from: "2021-01-01", to: "2021-12-31" });
  });

  it("carries the lens filter through the URL and its chip", () => {
    const p = paramsFromUrl(new URLSearchParams("lens=RF%2024-70mm%20F2.8%20L&camera=Canon%20EOS%20R5"));
    expect(p).toEqual({ lens: "RF 24-70mm F2.8 L", camera: "Canon EOS R5" });
    expect(paramsFromUrl(new URLSearchParams(urlFromParams(p)))).toEqual(p);
    expect(chipsOf(p).map((c) => c.label)).toEqual(["Canon EOS R5", "RF 24-70mm F2.8 L"]);
    expect(chipsOf(p)[1]!.clears).toEqual(["lens"]);
  });

  it("names the people of the people filter, in one chip that clears it", () => {
    const a = "6f1c2b0e-0000-4000-8000-000000000001";
    const b = "6f1c2b0e-0000-4000-8000-000000000002";
    const people = [
      { id: a, name: "Person A" },
      { id: b, name: "Person B" },
    ];
    const chips = chipsOf({ people: `${a},${b}`, place: "Lisbon" }, undefined, people);
    expect(chips.map((c) => c.label)).toEqual(["Lisbon", "Person A and Person B"]);
    expect(chips[1]!.clears).toEqual(["people"]);
    expect(chipsOf({ people: a }, undefined, [])[0]!.label).toBe("Unknown person");
  });

  it("makes one chip per filter and says which parameters it clears", () => {
    const chips = chipsOf(
      { from: "2024-01-01", to: "2024-12-31", favorite: true, camera: "Canon EOS R5", place: "Lisbon", type: "heic", has_location: true, album: "a" },
      "Pets",
    );
    expect(chips.map((c) => c.label)).toEqual(["2024", "Favorites", "Canon EOS R5", "Lisbon", "HEIC", "Has location", "Album: Pets"]);
    expect(chips[0]!.clears).toEqual(["from", "to"]);
    expect(chipsOf({ has_location: false })[0]!.label).toBe("No location");
    expect(chipsOf({ album: "a" })[0]!.label).toBe("Album");
    expect(chipsOf({})).toEqual([]);
  });
});

describe("indexing note", () => {
  const base: Processing = {
    active: false,
    queued: 0,
    running: 0,
    failed: 0,
    by_kind: [],
    total_photos: 10,
    without_thumbnail: 0,
    failures: [],
  };
  const with_ = (embedding: Processing["embedding"], over: Partial<Processing> = {}): Processing => ({ ...base, ...over, embedding });

  it("says nothing when everything is indexed or the server does not report it", () => {
    expect(indexingNote(null)).toBeNull();
    expect(indexingNote(base)).toBeNull();
    expect(indexingNote(with_({ indexed: 10, total: 10, queued: 0, running: 0, failed: 0 }))).toBeNull();
    expect(indexingPercent(with_({ indexed: 10, total: 10, queued: 0, running: 0, failed: 0 }))).toBeNull();
  });

  it("counts what is left while the worker is on it", () => {
    const p = with_({ indexed: 1000, total: 51234, queued: 50000, running: 1, failed: 0 });
    expect(indexingNote(p)).toBe("Still indexing 50,234 photos: results may be incomplete.");
    expect(indexingPercent(p)).toBe(1);
    expect(indexingNote(with_({ indexed: 9, total: 10, queued: 1, running: 0, failed: 0 }))).toBe(
      "Still indexing 1 photo: results may be incomplete.",
    );
  });

  it("says re-indexing, not 'incomplete', while an older model answers searches", () => {
    const e = {
      indexed: 400,
      total: 1000,
      queued: 600,
      running: 0,
      failed: 0,
      model: "siglip2-b16",
      percent: 40,
      reindexing: true,
      switch_at_percent: 95,
      serving_model: "clip-b32",
    };
    const p = with_(e);
    expect(indexingNote(p)).toBeNull();
    expect(reindexNote(p)).toBe("Re-indexing with the improved search model: 40%. Searches use the previous model until 95%.");
    expect(reindexNote(with_({ ...e, reindexing: false }))).toBeNull();
    expect(reindexNote(with_({ ...e, percent: undefined, switch_at_percent: undefined }))).toBe(
      "Re-indexing with the improved search model: 0%. Searches use the previous model until 95%.",
    );
    expect(reindexNote(null)).toBeNull();
    // Once the new model answers, the usual note applies again if photos are still missing.
    expect(indexingNote(with_({ ...e, reindexing: false, percent: 96 }))).toBe("Still indexing 600 photos: results may be incomplete.");
  });

  it("does not claim indexing when nothing is queued: thumbnails come first, else it is not searchable", () => {
    const e = { indexed: 0, total: 5, queued: 0, running: 0, failed: 0 };
    expect(indexingNote(with_(e, { without_thumbnail: 5 }))).toBeNull();
    expect(indexingNote(with_(e))).toBe("5 photos are not searchable yet.");
  });
});

describe("recent searches", () => {
  it("keeps the newest first, once, case-insensitively, up to a limit", () => {
    const s = memory();
    pushRecent("dog", s);
    pushRecent("cat", s);
    expect(pushRecent("DOG", s)).toEqual(["DOG", "cat"]);
    for (let i = 0; i < 20; i++) pushRecent(`q${i}`, s);
    expect(loadRecent(s)).toHaveLength(RECENT_MAX);
    expect(loadRecent(s)[0]).toBe("q19");
    expect(pushRecent("   ", s)[0]).toBe("q19");
    clearRecent(s);
    expect(loadRecent(s)).toEqual([]);
  });

  it("survives corrupt or unavailable storage", () => {
    expect(loadRecent({ getItem: () => "{not json" })).toEqual([]);
    expect(loadRecent({ getItem: () => '{"a":1}' })).toEqual([]);
    expect(loadRecent({ getItem: () => '["a",3,"b"]' })).toEqual(["a", "b"]);
    expect(
      pushRecent("x", {
        getItem: () => null,
        setItem: () => {
          throw new Error("quota");
        },
      }),
    ).toEqual(["x"]);
  });

  it("suggests recent searches that continue what is typed", () => {
    const recent = ["dog on the beach", "dogs", "cat"];
    expect(suggestionsFor("", recent)).toEqual(recent);
    expect(suggestionsFor("do", recent)).toEqual(["dog on the beach", "dogs"]);
    expect(suggestionsFor("DOGS", recent)).toEqual([]);
    expect(suggestionsFor("zebra", recent)).toEqual([]);
  });
});

describe("date shortcuts", () => {
  it("counts back from today, inclusive, and wraps the year", () => {
    expect(dateShortcuts("2026-03-10")).toEqual([
      { label: "Last 7 days", from: "2026-03-04", to: "2026-03-10" },
      { label: "This month", from: "2026-03-01", to: "2026-03-10" },
      { label: "Last month", from: "2026-02-01", to: "2026-02-28" },
      { label: "This year", from: "2026-01-01", to: "2026-03-10" },
      { label: "Last year", from: "2025-01-01", to: "2025-12-31" },
    ]);
    const jan = dateShortcuts("2026-01-03");
    expect(jan[0]).toEqual({ label: "Last 7 days", from: "2025-12-28", to: "2026-01-03" });
    expect(jan[2]).toEqual({ label: "Last month", from: "2025-12-01", to: "2025-12-31" });
  });
});

describe("quick filters", () => {
  const base = { favorites: 0, with_location: 0, types: [] as { value: string; count: number }[] };
  it("offers only what would find something, and only filters the search takes", () => {
    expect(quickFilters(base)).toEqual([]);
    const all = quickFilters({
      favorites: 3,
      with_location: 10,
      with_text: 2,
      ratings: [{ value: "5", count: 4 }],
      types: [
        { value: "raw", count: 6 },
        { value: "heic", count: 0 },
        { value: "jpeg", count: 50 },
      ],
    });
    expect(all.map((f) => f.key)).toEqual(["favorite", "rating", "text", "location", "raw"]);
    expect(all.find((f) => f.key === "raw")?.patch).toEqual({ type: "raw" });
    expect(all.find((f) => f.key === "rating")?.label).toBe("5 stars");
  });
});
