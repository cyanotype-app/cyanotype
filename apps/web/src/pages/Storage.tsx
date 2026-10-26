import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, type MyUsage } from "../api";
import { bytesLabel } from "../adminText";
import { barShares, countLabel, derivedLabel } from "../storageText";
import { meters, resumeText, warningFor, type Meter } from "../limitsText";
import { Block, FormError, Group, Loading, Notice, Row, Section } from "../settings/kit";
import { cn } from "../ui/cn";

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
    <div>
      <FormError>{error}</FormError>
      {usage === null ? !error && <Loading /> : <UsageView usage={usage} />}
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
      <div className="rounded-card bg-card p-5 ring-1 ring-line">
        <p className="flex items-baseline gap-2">
          <strong className="text-[2rem] leading-none font-semibold tracking-tight">{bytesLabel(usage.stored_bytes)}</strong>
          <span className="text-muted">stored</span>
        </p>
        <div
          className="mt-4 flex h-2.5 overflow-hidden rounded-full bg-surface"
          role="img"
          aria-label={`${bytesLabel(usage.originals_bytes)} in photos, ${bytesLabel(usage.trash_bytes)} in the trash`}
        >
          <span className="h-full bg-accent transition-[width] duration-300 ease-soft" style={{ width: `${shares.photos}%` }} />
          <span className="h-full bg-muted/45 transition-[width] duration-300 ease-soft" style={{ width: `${shares.trash}%` }} />
        </div>
        <ul className="mt-3 flex flex-wrap gap-x-5 gap-y-1 text-[0.8125rem] text-muted">
          <li className="flex items-center gap-2">
            <span className="size-2.5 rounded-full bg-accent" aria-hidden="true" /> Photos and videos
          </li>
          <li className="flex items-center gap-2">
            <span className="size-2.5 rounded-full bg-muted/45" aria-hidden="true" /> Recently deleted
          </li>
        </ul>
      </div>

      {warnings.length > 0 && (
        <div className="mt-4 flex flex-col gap-2">
          {warnings.map((w) => (
            <Notice key={w.text} tone="warn" role="status">
              {w.text}
            </Notice>
          ))}
        </div>
      )}

      <Section title="Your limits">
        <Group>
          <Block>
            <p className="mb-2 text-[0.9375rem] font-medium">Storage</p>
            {m.quota ? <MeterView m={m.quota} label="Storage used" /> : <p className="text-[0.9375rem]">No limit</p>}
            {m.quota && <p className="mt-2 text-[0.8125rem] text-muted">Photos and the trash count; thumbnails do not.</p>}
          </Block>
          <Block>
            <p className="mb-2 text-[0.9375rem] font-medium">Uploads per day</p>
            {m.daily ? <MeterView m={m.daily} label="Uploaded in the last 24 hours" /> : <p className="text-[0.9375rem]">No limit</p>}
            {m.daily && usage.daily?.oldest_rolls_out_at && usage.daily.used_bytes > 0 && (
              <p className="mt-2 text-[0.8125rem] text-muted">
                Counts the last 24 hours. The first of it frees up at {resumeText(usage.daily.oldest_rolls_out_at)}.
              </p>
            )}
          </Block>
          <Row label="Largest file">{m.maxFile ? bytesLabel(m.maxFile) : "No limit"}</Row>
        </Group>
      </Section>

      <Section title="Breakdown">
        <Group>
          <Row label="Photos">
            <span>
              {countLabel(usage.photo_count, "photo")}
              {usage.raw_count > 0 && <span className="text-muted"> · {countLabel(usage.raw_count, "RAW file")}</span>}
            </span>
          </Row>
          <Row label="Originals">
            <span>
              {bytesLabel(usage.originals_bytes)}
              <span className="text-muted"> in {countLabel(usage.file_count, "file")}</span>
            </span>
          </Row>
          <Row
            label="Recently deleted"
            detail={
              <>
                Photos here still take space until they are deleted for good.{" "}
                <Link to="/trash" className="text-accent">
                  Open the trash
                </Link>
              </>
            }
          >
            <span>
              {bytesLabel(usage.trash_bytes)}
              <span className="text-muted"> in {countLabel(usage.trash_count, "file")}</span>
            </span>
          </Row>
          <Row
            label="Thumbnails and previews"
            detail={
              <>
                Made from your originals and not counted in what you store.
                {!usage.derived_complete && " Some of them have not been measured yet, so this is a lower bound."}
              </>
            }
          >
            {derivedLabel(usage.derived_bytes, usage.derived_complete)}
          </Row>
        </Group>
      </Section>

      <Section title="Uploads">
        <Group>
          <Row label="Last 24 hours">
            <span>
              {countLabel(up.count_24h, "file")}
              <span className="text-muted"> · {bytesLabel(up.bytes_24h)}</span>
            </span>
          </Row>
          <Row label="Last 30 days">
            <span>
              {countLabel(up.count_30d, "file")}
              <span className="text-muted"> · {bytesLabel(up.bytes_30d)}</span>
            </span>
          </Row>
        </Group>
      </Section>
    </>
  );
}

function MeterView({ m, label }: { m: Meter; label: string }) {
  return (
    <>
      <div
        className="h-2 overflow-hidden rounded-full bg-surface"
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={m.percent}
      >
        <span
          className={cn(
            "block h-full rounded-full transition-[width] duration-300 ease-soft",
            m.level === "ok" && "bg-accent",
            m.level === "warn" && "bg-danger/60",
            (m.level === "critical" || m.level === "full") && "bg-danger",
          )}
          style={{ width: `${m.percent}%` }}
        />
      </div>
      <p className="mt-1.5 text-[0.9375rem]">
        {bytesLabel(m.used)} of {bytesLabel(m.limit)} <span className="text-muted">({m.percent}%)</span>
      </p>
    </>
  );
}
