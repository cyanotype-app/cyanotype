import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type Tag } from "../api";
import { useAuth } from "../auth";
import { tagPhotos } from "../tagText";

/** The user's tags with how many photos have each: a filter box, alphabetical or most used first. Each opens its tag page. */
export function TagsPage() {
  const { setAnonymous } = useAuth();
  const [tags, setTags] = useState<Tag[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sort, setSort] = useState<"name" | "count">("name");
  const [filter, setFilter] = useState("");

  useEffect(() => {
    let live = true;
    api
      .listTags({ sort, limit: 5000 })
      .then((r) => live && setTags(r.items))
      .catch((e: unknown) => {
        if (!live) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else setError(e instanceof Error ? e.message : "Could not load your tags.");
      });
    return () => {
      live = false;
    };
  }, [sort, setAnonymous]);

  const shown = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return (tags ?? []).filter((t) => !f || t.name.toLowerCase().includes(f));
  }, [tags, filter]);

  return (
    <div className="tags-page">
      <div className="page-head">
        <h1>Tags</h1>
        <div className="page-head-actions">
          <div className="segmented" role="group" aria-label="Order">
            {(["name", "count"] as const).map((s) => (
              <button key={s} type="button" className="btn btn-quiet" aria-pressed={sort === s} onClick={() => setSort(s)}>
                {s === "name" ? "A to Z" : "Most used"}
              </button>
            ))}
          </div>
        </div>
      </div>
      {tags !== null && tags.length > 0 && (
        <div className="tags-filter">
          <label htmlFor="tags-filter" className="sr-only">
            Find a tag
          </label>
          <input
            id="tags-filter"
            className="text-input"
            type="search"
            placeholder="Find a tag"
            value={filter}
            maxLength={100}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
      )}
      {error && (
        <p className="center" role="alert">
          {error}
        </p>
      )}
      {!error && tags === null && (
        <p className="center muted" role="status">
          Loading your tags…
        </p>
      )}
      {tags !== null && tags.length === 0 && (
        <div className="empty">
          <h2>No tags yet</h2>
          <p>Select photos and choose Tag, or open a photo&rsquo;s info and add a tag. Keywords stored in your photos are imported as tags.</p>
        </div>
      )}
      {tags !== null && tags.length > 0 && shown.length === 0 && <p className="center muted">No tag has “{filter.trim()}” in its name.</p>}
      {shown.length > 0 && (
        <ul className="tag-list" aria-label="Tags">
          {shown.map((t) => (
            <li key={t.id}>
              <Link to={`/tags/${t.id}`} className="tag-row">
                <span className="tag-row-name">{t.name}</span>
                <span className="muted small tag-row-count">{tagPhotos(t.count)}</span>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
