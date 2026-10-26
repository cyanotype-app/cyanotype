import { useId, useMemo, useRef, useState, type DragEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { filesFromDrop } from "../upload/files";
import { useUploads } from "../upload/UploadProvider";
import { planFiles, previewOf, uploadSet, type Options } from "../takeout";
import type { TakeoutPlan } from "../api";

type Phase = "idle" | "planning" | "ready" | "started";

const count = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

/** Settings > Google Photos: import an unzipped Google Takeout folder with its metadata, from the browser. */
export function TakeoutImport() {
  const { queue } = useUploads();
  const favId = useId();
  const albumId = useId();
  const input = useRef<HTMLInputElement>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [files, setFiles] = useState<File[]>([]);
  const [plan, setPlan] = useState<TakeoutPlan | null>(null);
  const [opts, setOpts] = useState<Options>({ favorites: true, albums: true });
  const [error, setError] = useState<string | null>(null);
  const [over, setOver] = useState(false);
  const [queued, setQueued] = useState(0);
  const preview = useMemo(() => (plan ? previewOf(plan) : null), [plan]);

  async function choose(chosen: File[]) {
    setError(null);
    if (chosen.length === 0) return;
    if (chosen.every((f) => f.name.toLowerCase().endsWith(".zip"))) {
      setError(
        "These are zip files. Unzip them first (all of them into the same folder), then choose that folder. Browsers cannot read zip files of this size piece by piece.",
      );
      return;
    }
    setPhase("planning");
    try {
      const p = await planFiles(chosen, api.takeoutPlan);
      setFiles(chosen);
      setPlan(p);
      setPhase("ready");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not read the folder.");
      setPhase("idle");
    }
  }

  function start() {
    if (!plan) return;
    const set = uploadSet(plan, files, opts);
    queue.add(set.files, set.takeoutOf);
    setQueued(set.files.length);
    setPhase("started");
  }

  function reset() {
    setPhase("idle");
    setPlan(null);
    setFiles([]);
    if (input.current) input.current.value = "";
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    setOver(false);
    void filesFromDrop(e.dataTransfer).then(choose);
  }

  return (
    <div className="settings takeout">
      <h1>Moving from Google Photos</h1>
      <p className="muted">
        Bring your library over with its dates, places, captions, favorites and albums. Request an export at{" "}
        <strong>takeout.google.com</strong> (only <em>Google Photos</em>, as .zip files), download and{" "}
        <strong>unzip all of them into one folder</strong>, then choose that folder here. Nothing is uploaded until you
        confirm.
      </p>

      {phase === "idle" || phase === "planning" ? (
        <div
          className={`dropzone${over ? " over" : ""}`}
          onDragOver={(e) => {
            e.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={onDrop}
        >
          <p>Drop the Takeout folder here</p>
          <input
            ref={input}
            type="file"
            multiple
            hidden
            aria-label="Takeout folder"
            {...{ webkitdirectory: "" }}
            onChange={(e) => void choose(Array.from(e.target.files ?? []))}
          />
          <button
            type="button"
            className="btn btn-primary"
            disabled={phase === "planning"}
            onClick={() => input.current?.click()}
          >
            {phase === "planning" ? "Reading the folder…" : "Choose the Takeout folder"}
          </button>
        </div>
      ) : null}

      <div role="alert" className="form-error">
        {error}
      </div>

      {phase === "ready" && preview && (
        <section className="preview" aria-label="What will be imported">
          <h2>{count(preview.photos, "photo")} to import</h2>
          <ul>
            <li>
              {count(preview.withSidecar, "photo")} with their Google Photos details (date, place, caption, favorite)
            </li>
            <li>
              {count(preview.withoutSidecar, "photo")} without a details file: imported as they are
              {preview.edited > 0 && `, ${count(preview.edited, "edited copy", "edited copies")} sharing their original's`}
            </li>
            <li>
              {count(preview.albums.length, "album")} from named folders
              {preview.albums.length > 0 && (
                <ul className="album-names">
                  {preview.albums.slice(0, 8).map((a) => (
                    <li key={a.name}>
                      <span className="album-name">{a.name}</span> <span className="muted">({count(a.photos, "photo")})</span>
                    </li>
                  ))}
                  {preview.albums.length > 8 && <li className="muted">and {preview.albums.length - 8} more</li>}
                </ul>
              )}
            </li>
            <li className="muted">
              Skipped: {count(preview.videos, "video")} (Cyanotype does not import video)
              {preview.trashed > 0 && `, ${count(preview.trashed, "photo")} in Google Photos' trash`}
              {preview.other > 0 && `, ${count(preview.other, "other file")}`}
            </li>
          </ul>

          <fieldset className="options">
            <legend>Options</legend>
            <label htmlFor={favId} className="check">
              <input
                id={favId}
                type="checkbox"
                checked={opts.favorites}
                onChange={(e) => setOpts({ ...opts, favorites: e.target.checked })}
              />{" "}
              Import favorites
            </label>
            <label htmlFor={albumId} className="check">
              <input
                id={albumId}
                type="checkbox"
                checked={opts.albums}
                onChange={(e) => setOpts({ ...opts, albums: e.target.checked })}
              />{" "}
              Create albums from folders
            </label>
          </fieldset>
          <p className="muted small">
            Photos already in your library are not uploaded again: they just get their details. You can run this again
            safely, and undo any date, place or caption with "Revert to original".
          </p>
          <div className="actions">
            <button type="button" className="btn btn-primary" disabled={preview.photos === 0} onClick={start}>
              Import {count(preview.photos, "photo")}
            </button>
            <button type="button" className="btn btn-quiet" onClick={reset}>
              Choose another folder
            </button>
          </div>
        </section>
      )}

      {phase === "started" && (
        <section className="preview" role="status" aria-label="Import started">
          <h2>Importing {count(queued, "photo")}</h2>
          <p>
            Progress is in the box at the bottom of the page, and you can keep using Cyanotype meanwhile. Keep this tab open
            while photos upload. Dates, places, captions, favorites and albums are added by the server once each photo is
            processed, so they do not depend on this tab.
          </p>
          <p className="muted small">
            If you close the tab early, choose the same folder again: photos that arrived are not uploaded twice, and they
            still get their details.
          </p>
          <div className="actions">
            <Link className="btn btn-primary" to="/photos">
              Go to the library
            </Link>
            <button type="button" className="btn btn-quiet" onClick={reset}>
              Import another folder
            </button>
          </div>
        </section>
      )}
    </div>
  );
}
