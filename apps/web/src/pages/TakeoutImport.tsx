import { useMemo, useRef, useState, type DragEvent } from "react";
import { Link } from "react-router-dom";
import { FolderUp } from "lucide-react";
import { api } from "../api";
import { filesFromDrop } from "../upload/files";
import { useUploads } from "../upload/UploadProvider";
import { planFiles, previewOf, uploadSet, type Options } from "../takeout";
import type { TakeoutPlan } from "../api";
import { Block, FormError, Group, Section } from "../settings/kit";
import { Button } from "../ui/Button";
import { Switch } from "../ui/Controls";
import { cn } from "../ui/cn";

type Phase = "idle" | "planning" | "ready" | "started";

const count = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

/** Settings > Google Photos: import an unzipped Google Takeout folder with its metadata, from the browser. */
export function TakeoutImport() {
  const { queue } = useUploads();
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
    <div>
      <p className="text-sm text-muted">
        Bring your library over with its dates, places, captions, favorites and albums. Request an export at{" "}
        <strong className="font-semibold text-fg">takeout.google.com</strong> (only <em>Google Photos</em>, as .zip files), download and{" "}
        <strong className="font-semibold text-fg">unzip all of them into one folder</strong>, then choose that folder here. Nothing is
        uploaded until you confirm.
      </p>

      {phase === "idle" || phase === "planning" ? (
        <div
          className={cn(
            "mt-5 flex flex-col items-center gap-3 rounded-card px-6 py-10 text-center transition-colors duration-150",
            over ? "bg-accent/10 ring-2 ring-accent" : "bg-card ring-1 ring-line",
          )}
          onDragOver={(e) => {
            e.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={onDrop}
        >
          <FolderUp className="size-8 text-muted" aria-hidden="true" />
          <p className="text-[0.9375rem]">Drop the Takeout folder here</p>
          <input
            ref={input}
            type="file"
            multiple
            hidden
            aria-label="Takeout folder"
            {...{ webkitdirectory: "" }}
            onChange={(e) => void choose(Array.from(e.target.files ?? []))}
          />
          <Button variant="primary" disabled={phase === "planning"} onClick={() => input.current?.click()}>
            {phase === "planning" ? "Reading the folder…" : "Choose the Takeout folder"}
          </Button>
        </div>
      ) : null}

      <div className="mt-3">
        <FormError>{error}</FormError>
      </div>

      {phase === "ready" && preview && (
        <section aria-label="What will be imported" className="mt-5">
          <h2 className="text-[1.0625rem] font-semibold tracking-tight">{count(preview.photos, "photo")} to import</h2>
          <Group as="ul" className="mt-3 text-[0.9375rem]">
            <li className="px-4 py-3">
              {count(preview.withSidecar, "photo")} with their Google Photos details (date, place, caption, favorite)
            </li>
            <li className="px-4 py-3">
              {count(preview.withoutSidecar, "photo")} without a details file: imported as they are
              {preview.edited > 0 && `, ${count(preview.edited, "edited copy", "edited copies")} sharing their original's`}
            </li>
            <li className="px-4 py-3">
              {count(preview.albums.length, "album")} from named folders
              {preview.albums.length > 0 && (
                <ul className="mt-2 flex flex-col gap-1 text-[0.8125rem]">
                  {preview.albums.slice(0, 8).map((a) => (
                    <li key={a.name}>
                      <span className="font-medium">{a.name}</span> <span className="text-muted">({count(a.photos, "photo")})</span>
                    </li>
                  ))}
                  {preview.albums.length > 8 && <li className="text-muted">and {preview.albums.length - 8} more</li>}
                </ul>
              )}
            </li>
            <li className="px-4 py-3 text-muted">
              Skipped: {count(preview.videos, "video")} (Cyanotype does not import video)
              {preview.trashed > 0 && `, ${count(preview.trashed, "photo")} in Google Photos' trash`}
              {preview.other > 0 && `, ${count(preview.other, "other file")}`}
            </li>
          </Group>

          <Section title="Options" className="mt-6">
            <Group>
              <Block>
                <Switch label="Import favorites" checked={opts.favorites} onChange={(on) => setOpts({ ...opts, favorites: on })} />
              </Block>
              <Block>
                <Switch label="Create albums from folders" checked={opts.albums} onChange={(on) => setOpts({ ...opts, albums: on })} />
              </Block>
            </Group>
          </Section>
          <p className="mt-3 text-[0.8125rem] text-muted">
            Photos already in your library are not uploaded again: they just get their details. You can run this again safely, and undo any
            date, place or caption with &quot;Revert to original&quot;.
          </p>
          <div className="mt-5 flex flex-wrap gap-2">
            <Button variant="primary" disabled={preview.photos === 0} onClick={start}>
              Import {count(preview.photos, "photo")}
            </Button>
            <Button variant="quiet" onClick={reset}>
              Choose another folder
            </Button>
          </div>
        </section>
      )}

      {phase === "started" && (
        <section role="status" aria-label="Import started" className="mt-5 rounded-card bg-card p-5 ring-1 ring-line">
          <h2 className="text-[1.0625rem] font-semibold tracking-tight">Importing {count(queued, "photo")}</h2>
          <p className="mt-2 text-[0.9375rem]">
            Progress is in the box at the bottom of the page, and you can keep using Cyanotype meanwhile. Keep this tab open while photos
            upload. Dates, places, captions, favorites and albums are added by the server once each photo is processed, so they do not
            depend on this tab.
          </p>
          <p className="mt-2 text-[0.8125rem] text-muted">
            If you close the tab early, choose the same folder again: photos that arrived are not uploaded twice, and they still get their
            details.
          </p>
          <div className="mt-4 flex flex-wrap gap-2">
            <Button variant="primary" asChild>
              <Link to="/photos">Go to the library</Link>
            </Button>
            <Button variant="quiet" onClick={reset}>
              Import another folder
            </Button>
          </div>
        </section>
      )}
    </div>
  );
}
