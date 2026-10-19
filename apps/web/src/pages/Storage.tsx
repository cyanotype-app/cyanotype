import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, type MyUsage } from "../api";
import { bytesLabel } from "../adminText";
import { barShares, countLabel, derivedLabel } from "../storageText";
import { meters, resumeText, warningFor, type Meter } from "../limitsText";

/** Settings > Storage: what the account stores, split into photos and trash, and what it uploaded lately. */
export function Storage() {
  const [usage, setUsage] = useState<MyUsage | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .usage()
      .then((u) => live && setUsage(u))
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : "Could not load your storage."));
    return () => {
      live = false;
    };
  }, []);

  return (
    <div className="settings storage">
      <h1>Storage</h1>
      <div role="alert" className="form-error">
        {error}
      </div>
      {usage === null ? (
        !error && (
          <p className="muted" role="status">
            Loading…
          </p>
        )
      ) : (
        <UsageView usage={usage} />
      )}
    </div>
  );
}

function UsageView({ usage }: { usage: MyUsage }) {
  const shares = barShares(usage.originals_bytes, usage.trash_bytes);
  const up = usage.uploads;
  const m = meters(usage);
  const warnings = [
    { text: warningFor(m.quota, "storage"), level: m.quota?.level },
    { text: warningFor(m.daily, "daily upload limit"), level: m.daily?.level },
  ].filter((w): w is { text: string; level: Meter["level"] } => w.text !== null && w.level !== undefined);
  return (
    <>
      <p className="storage-total">
        <strong>{bytesLabel(usage.stored_bytes)}</strong> <span className="muted">stored</span>
      </p>
      <div
        className="usage-bar"
        role="img"
        aria-label={`${bytesLabel(usage.originals_bytes)} in photos, ${bytesLabel(usage.trash_bytes)} in the trash`}
      >
        <span className="usage-photos" style={{ width: `${shares.photos}%` }} />
        <span className="usage-trash" style={{ width: `${shares.trash}%` }} />
      </div>
      <ul className="usage-legend">
        <li>
          <span className="swatch usage-photos" aria-hidden="true" /> Photos and videos
        </li>
        <li>
          <span className="swatch usage-trash" aria-hidden="true" /> Trash
        </li>
      </ul>

      {warnings.map((w) => (
        <p key={w.text} className={`limit-warning level-${w.level}`} role="status">
          {w.text}
        </p>
      ))}

      <h2>Your limits</h2>
      <dl className="usage-list">
        <dt>Storage</dt>
        <dd>
          {m.quota ? <MeterView m={m.quota} label="Storage used" /> : "No limit"}
          {m.quota && <div className="muted small">Photos and the trash count; thumbnails do not.</div>}
        </dd>
        <dt>Uploads per day</dt>
        <dd>
          {m.daily ? <MeterView m={m.daily} label="Uploaded in the last 24 hours" /> : "No limit"}
          {m.daily && usage.daily?.oldest_rolls_out_at && usage.daily.used_bytes > 0 && (
            <div className="muted small">
              Counts the last 24 hours. The first of it frees up at {resumeText(usage.daily.oldest_rolls_out_at)}.
            </div>
          )}
        </dd>
        <dt>Largest file</dt>
        <dd>{m.maxFile ? bytesLabel(m.maxFile) : "No limit"}</dd>
      </dl>

      <h2>Breakdown</h2>
      <dl className="usage-list">
        <dt>Photos</dt>
        <dd>
          {countLabel(usage.photo_count, "photo")}
          {usage.raw_count > 0 && <span className="muted"> · {countLabel(usage.raw_count, "RAW file")}</span>}
        </dd>
        <dt>Originals</dt>
        <dd>
          {bytesLabel(usage.originals_bytes)}
          <span className="muted"> in {countLabel(usage.file_count, "file")}</span>
        </dd>
        <dt>Trash</dt>
        <dd>
          {bytesLabel(usage.trash_bytes)}
          <span className="muted"> in {countLabel(usage.trash_count, "file")}</span>
          <div className="muted small">
            Photos in the trash still take space until they are deleted for good. <Link to="/trash">Open the trash</Link>
          </div>
        </dd>
        <dt>Thumbnails and previews</dt>
        <dd>
          {derivedLabel(usage.derived_bytes, usage.derived_complete)}
          <div className="muted small">
            Made from your originals and not counted in what you store.
            {!usage.derived_complete && " Some of them have not been measured yet, so this is a lower bound."}
          </div>
        </dd>
      </dl>

      <h2>Uploads</h2>
      <dl className="usage-list">
        <dt>Last 24 hours</dt>
        <dd>
          {countLabel(up.count_24h, "file")}
          <span className="muted"> · {bytesLabel(up.bytes_24h)}</span>
        </dd>
        <dt>Last 30 days</dt>
        <dd>
          {countLabel(up.count_30d, "file")}
          <span className="muted"> · {bytesLabel(up.bytes_30d)}</span>
        </dd>
      </dl>
    </>
  );
}

function MeterView({ m, label }: { m: Meter; label: string }) {
  return (
    <>
      <div
        className={`meter level-${m.level}`}
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={m.percent}
      >
        <span style={{ width: `${m.percent}%` }} />
      </div>
      <div>
        {bytesLabel(m.used)} of {bytesLabel(m.limit)} <span className="muted">({m.percent}%)</span>
      </div>
    </>
  );
}
