import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, type Memories } from "../api";
import { MemoryCard } from "../collections/cards";
import { ScrollRow } from "../collections/ScrollRow";
import { hasMemories, memoryCount, yearsAgo } from "../viewText";

/**
 * "On this day" as a row of cards: one per earlier year, with that year's first photo, "3 years ago" and how many photos
 * there are. Tapping a card opens the page with all of them. Shows nothing at all when there is nothing for today (no
 * empty strip), when the user turned it off, or when the request fails. (Collections draws the same cards together with
 * the recent trips; this stand-alone strip is for a page that wants only On this day.)
 */
export function MemoriesStrip() {
  const [memories, setMemories] = useState<Memories | null>(null);
  const [gone, setGone] = useState(false);
  useEffect(() => {
    let live = true;
    api
      .memories()
      .then((m) => live && setMemories(m))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);
  if (gone || !memories || !memories.enabled || !hasMemories(memories.years)) return null;
  return (
    <section aria-label="On this day" className="pt-3">
      <div className="mb-3 flex items-baseline gap-2 px-4 md:px-8">
        <h2 className="flex-1 text-[1.0625rem] font-semibold tracking-tight">On this day</h2>
        <Link to="/memories" className="rounded-full px-2 py-2 text-[0.9375rem] text-accent no-underline hover:bg-surface">
          See all
        </Link>
        <button
          type="button"
          aria-label="Hide On this day"
          className="rounded-full px-2 py-2 text-[0.9375rem] text-muted hover:bg-surface hover:text-fg"
          onClick={() => {
            setGone(true);
            void api.setPrefs({ memories_enabled: false });
          }}
        >
          Hide
        </button>
      </div>
      <ScrollRow label="On this day cards">
        {memories.years.map((y) => (
          <MemoryCard
            key={y.year}
            to={`/memories#y${y.year}`}
            title={yearsAgo(y.years_ago)}
            subtitle={memoryCount(y.count)}
            name={`${yearsAgo(y.years_ago)}, ${memoryCount(y.count)}`}
            cover={y.items[0] ?? null}
          />
        ))}
      </ScrollRow>
    </section>
  );
}
