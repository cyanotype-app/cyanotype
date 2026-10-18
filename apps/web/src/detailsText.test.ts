import { describe, expect, it } from "vitest";
import {
  burstText,
  coordinatesText,
  dayFromSearch,
  dayHref,
  mapHref,
  pointFromSearch,
  resolutionText,
  searchHref,
  whenText,
  zoneText,
} from "./detailsText";
import { photoDetails } from "./test/fixtures";

describe("whenText", () => {
  it("prints the photo's own wall clock whatever the browser's zone is, with its UTC offset", () => {
    const w = whenText(photoDetails().date);
    expect(w).toEqual({ day: "Tuesday, July 4, 2023", time: "1:05 PM", zone: "UTC+02:00", note: null });
  });

  it("says when the time zone is unknown", () => {
    const w = whenText({ ...photoDetails().date, offset: null, zone_known: false });
    expect(w.zone).toBe("Time zone unknown");
    expect(w.time).toBe("1:05 PM");
  });

  it("names the fallbacks for a photo without a capture time", () => {
    expect(whenText({ ...photoDetails().date, basis: "file" }).note).toMatch(/modification date/);
    expect(whenText({ ...photoDetails().date, basis: "added" }).note).toMatch(/when it was added/);
  });

  it("zones", () => {
    expect(zoneText("+00:00")).toBe("UTC");
    expect(zoneText("-07:00")).toBe("UTC-07:00");
    expect(zoneText(null)).toBeNull();
  });
});

describe("small texts and links", () => {
  it("coordinates name the hemisphere", () => {
    expect(coordinatesText(38.7223, -9.1393)).toBe("38.7223° N, 9.1393° W");
    expect(coordinatesText(-33.86, 151.2)).toBe("33.8600° S, 151.2000° E");
  });
  it("burst membership", () => expect(burstText({ position: 1, size: 8 })).toBe("1 of 8 in a burst"));
  it("resolution with megapixels, or nothing without a size", () => {
    expect(resolutionText({ width: 4032, height: 3024, megapixels: "12 MP" })).toBe("4032 × 3024 · 12 MP");
    expect(resolutionText({ width: 10, height: 10, megapixels: null })).toBe("10 × 10");
    expect(resolutionText({ width: null, height: null, megapixels: null })).toBeNull();
  });
  it("shortcuts go to the search, the map and the timeline", () => {
    expect(searchHref({ camera: "Apple iPhone 15" })).toBe("/search?camera=Apple+iPhone+15");
    expect(searchHref({ lens: "RF 24-70mm F2.8 L" })).toBe("/search?lens=RF+24-70mm+F2.8+L");
    expect(searchHref({ place: "Lisbon" })).toBe("/search?place=Lisbon");
    expect(mapHref(38.7, -9.1)).toBe("/map?lat=38.7&lon=-9.1");
    expect(dayHref("2023-07-04")).toBe("/library?day=2023-07-04");
  });
  it("reads the map and library parameters strictly", () => {
    expect(pointFromSearch(new URLSearchParams("lat=38.7&lon=-9.1"))).toEqual({ lat: 38.7, lon: -9.1 });
    expect(pointFromSearch(new URLSearchParams("lat=91&lon=0"))).toBeNull();
    expect(pointFromSearch(new URLSearchParams("lat=x&lon=0"))).toBeNull();
    expect(pointFromSearch(new URLSearchParams("lat=&lon="))).toBeNull();
    expect(pointFromSearch(new URLSearchParams(""))).toBeNull();
    expect(dayFromSearch(new URLSearchParams("day=2023-07-04"))).toBe("2023-07-04");
    expect(dayFromSearch(new URLSearchParams("day=2023-13-45"))).toBeNull();
    expect(dayFromSearch(new URLSearchParams("day=yesterday"))).toBeNull();
  });
});
