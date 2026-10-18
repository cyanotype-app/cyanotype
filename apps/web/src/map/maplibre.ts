// Loads MapLibre GL and wires the pmtiles protocol and the worker once per page. Shared by the map page and the small
// map in the photo details panel; maplibre-gl is ~800 kB, so it is only imported when a map is about to be drawn.
let protocolReady = false;

export async function loadMapLibre(): Promise<typeof import("maplibre-gl")> {
  const [maplibre, pm, worker] = await Promise.all([
    import("maplibre-gl"),
    import("pmtiles"),
    // MapLibre 6 ships its worker as a separate module that imports a shared chunk; Vite bundles it into one
    // file and gives its URL, which has to be handed over (the default lookup fails in a bundled app).
    import("maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url"),
  ]);
  if (!protocolReady) {
    maplibre.setWorkerUrl(worker.default);
    maplibre.addProtocol("pmtiles", new pm.Protocol().tile);
    protocolReady = true;
  }
  return maplibre;
}
