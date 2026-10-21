import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, type Memories } from "../api";
import { placeholderUrl } from "../thumbhash";
import { hasMemories, memoryCount, yearsAgo } from "../viewText";

/**
 * "On this day" at the top of the library: one card per earlier year, with that year's first photo, "3 years ago" and how many
 * photos there are. Tapping a card opens the page with all of them. Shows nothing at all when there is nothing for today (no
 * empty strip), when the user turned it off, or when the request fails.
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
    <section className="memories" aria-label="On this day">
      <div className="memories-head">
        <h2>On this day</h2>
        <Link to="/memories" className="btn btn-quiet memories-all">
          See all
        </Link>
        <button
          type="button"
          className="btn btn-quiet"
          aria-label="Hide On this day"
          onClick={() => {
            setGone(true);
            void api.setPrefs({ memories_enabled: false });
          }}
        >
          Hide
        </button>
      </div>
      <ul className="memories-row">
        {memories.years.map((y) => {
          const cover = y.items[0]!;
          const bg = placeholderUrl(cover.thumbhash);
          return (
            <li key={y.year}>
              <Link
                to={`/memories#y${y.year}`}
                className="memory-card"
                style={bg ? { backgroundImage: `url(${bg})` } : undefined}
                aria-label={`${yearsAgo(y.years_ago)}, ${memoryCount(y.count)}`}
              >
                {cover.thumb_url && <img src={cover.thumb_url} alt="" loading="lazy" draggable={false} />}
                <span className="memory-label">
                  <strong>{yearsAgo(y.years_ago)}</strong>
                  <span>{memoryCount(y.count)}</span>
                </span>
              </Link>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
