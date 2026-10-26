import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, localToday, type Trip } from "../api";
import { AuthProvider } from "../auth";
import { UploadPickerProvider } from "../shell/UploadPicker";
import { shareApi, type ShareLink } from "../shareApi";
import { album, item } from "../test/fixtures";
import { UploadProvider } from "../upload/UploadProvider";
import { PeopleProvider } from "../usePeople";
import { Collections } from "./Collections";

const trip = (n: number, over: Partial<Trip> = {}): Trip => ({
  id: `2026-10-0${n}_2026-10-0${n + 1}`,
  from: `2026-10-0${n}`,
  to: `2026-10-0${n + 1}`,
  ongoing: false,
  title: `Place ${n}`,
  date_label: "October 2026",
  place: { name: `Place ${n}`, level: "city", city: `Place ${n}`, country: "France", country_code: "FR" },
  photos: 20 + n,
  cover_id: `id-${n}`,
  cover: item(n),
  ...over,
});

const link = (over: Partial<ShareLink> = {}): ShareLink => ({
  id: "s1",
  kind: "album",
  album: { id: "album-1", title: "Summer" },
  title: null,
  token: "tok",
  has_password: false,
  allow_download: true,
  include_originals: false,
  expires_at: null,
  expired: false,
  item_count: 12,
  view_count: 1,
  last_viewed_at: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  ...over,
});

interface Library {
  years?: { year: number; years_ago: number; count: number; items: ReturnType<typeof item>[] }[];
  trips?: Trip[];
  albums?: ReturnType<typeof album>[];
  recent?: ReturnType<typeof item>[];
  people?: boolean;
  shares?: ShareLink[];
}

function setup(lib: Library = {}) {
  vi.spyOn(api, "memories").mockResolvedValue({ enabled: true, day: "10-26", years: lib.years ?? [] });
  vi.spyOn(api, "trips").mockResolvedValue({ enabled: true, trips: lib.trips ?? [], next_before: null });
  vi.spyOn(api, "listAlbums").mockResolvedValue({ items: lib.albums ?? [] });
  vi.spyOn(api, "listAssets").mockResolvedValue({ items: lib.recent ?? [], next_cursor: null, next_before: null });
  vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: lib.people ?? false });
  vi.spyOn(api, "listPeople").mockResolvedValue({
    items: [
      {
        id: "p1",
        name: "Alpha",
        hidden: false,
        faces: 5,
        photos: 5,
        cover: { face_id: "f1", asset_id: "id-1", box: [0.1, 0.1, 0.3, 0.3], preview_url: "https://s3.test/p1" },
      },
    ],
  });
  vi.spyOn(shareApi, "list").mockResolvedValue({ items: lib.shares ?? [] });
  render(
    <MemoryRouter>
      <AuthProvider>
        <UploadProvider>
          <UploadPickerProvider>
            <PeopleProvider>
              <Collections />
            </PeopleProvider>
          </UploadPickerProvider>
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
});
afterEach(() => vi.restoreAllMocks());

const heading = (name: string) => screen.findByRole("heading", { level: 2, name });

describe("Collections", () => {
  it("invites a brand-new library to upload instead of showing empty sections", async () => {
    setup();
    expect(await heading("Start your library")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Upload photos" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Albums" })).toBeNull();
    // The way into the rest stays.
    expect(screen.getByRole("link", { name: "Recently deleted" })).toHaveAttribute("href", "/trash");
  });

  it("shows On this day and a fresh trip as memories, and the recent places with a link to the map", async () => {
    const today = localToday();
    setup({
      years: [{ year: 2023, years_ago: 3, count: 9, items: [item(1)] }],
      trips: [trip(1, { ongoing: true, to: today, id: `2026-10-01_${today}` }), trip(2, { to: "2020-01-01" })],
      albums: [album(1), album(2, { title: "Beach" })],
      recent: [item(5), item(6)],
    });
    const memories = within(await screen.findByRole("region", { name: "Memories" }));
    expect(memories.getByRole("link", { name: "3 years ago, 9 photos" })).toHaveAttribute("href", "/memories#y2023");
    expect(memories.getByRole("link", { name: "Place 1, October 2026, 21 photos" })).toHaveAttribute(
      "href",
      "/trips/2026-10-01_" + today,
    );
    // An old trip is not a memory, but it is a recent place.
    expect(memories.queryByRole("link", { name: /Place 2/ })).toBeNull();
    const places = within(await screen.findByRole("region", { name: "Recent places" }));
    expect(places.getAllByRole("link").map((a) => a.getAttribute("aria-label"))).toEqual(["Open the map of your places", "Place 1, October 2026, 21 photos", "Place 2, October 2026, 22 photos"]);
    expect(screen.getByRole("link", { name: "Open the map of your places" })).toHaveAttribute("href", "/map");
  });

  it("lists albums with their counts, and the latest photos with a link to all of them", async () => {
    setup({ albums: [album(3), album(1)], recent: [item(5)] });
    const albums = within(await screen.findByRole("region", { name: "Albums" }));
    expect(albums.getByRole("link", { name: "Album 3, 3 photos" })).toHaveAttribute("href", "/albums/album-3");
    expect(albums.getByRole("link", { name: "See all" })).toHaveAttribute("href", "/albums");
    const recent = within(await screen.findByRole("region", { name: "Recently added" }));
    expect(recent.getByRole("link", { name: "See all" })).toHaveAttribute("href", "/recent");
  });

  it("offers to make a first album when there are photos but no albums", async () => {
    setup({ recent: [item(1)] });
    expect(await screen.findByRole("link", { name: "Make an album" })).toHaveAttribute("href", "/albums");
  });

  it("shows People only when face recognition is on, and shared links only while some are live", async () => {
    setup({ people: true, recent: [item(1)], shares: [link(), link({ id: "s2", expired: true, title: "Old" })] });
    const people = within(await screen.findByRole("region", { name: "People" }));
    expect(people.getByRole("link", { name: "Alpha, 5 photos" })).toHaveAttribute("href", "/people/p1");
    const shared = within(await screen.findByRole("region", { name: "Shared links" }));
    expect(shared.getAllByRole("listitem")).toHaveLength(1);
    expect(shared.getByText("Summer")).toBeInTheDocument();
  });

  it("hides People and shared links when there is nothing for them", async () => {
    setup({ recent: [item(1)] });
    await heading("Recently added");
    expect(screen.queryByRole("region", { name: "People" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Shared links" })).toBeNull();
  });

  it("lists the remaining ways in at the bottom", async () => {
    setup({ recent: [item(1)] });
    const library = within(await screen.findByRole("region", { name: "Library" }));
    expect(library.getAllByRole("link").map((a) => a.textContent)).toEqual(["Places", "Memories", "Favorites", "Tags", "Shared links"]);
    const utilities = within(screen.getByRole("region", { name: "Utilities" }));
    expect(utilities.getAllByRole("link").map((a) => a.textContent)).toEqual(["Archive", "Hidden", "Duplicates", "Recently deleted"]);
  });
});
