import { Search, Tag as TagIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, type Tag } from "../api";
import { useAuth } from "../auth";
import { Empty, InlineError, Loading, Rise } from "../components/pageKit";
import { inputClass } from "../components/sheetForm";
import { PageHeader } from "../shell/PageHeader";
import { Segmented } from "../ui/Controls";
import { cn } from "../ui/cn";
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
  const some = tags !== null && tags.length > 0;

  return (
    <>
      <PageHeader title="Tags" subtitle={some ? `${tags.length.toLocaleString("en-US")} ${tags.length === 1 ? "tag" : "tags"}` : undefined}>
        {some && (
          <div className="flex flex-wrap items-center gap-2">
            <div className="relative min-w-0 flex-1 basis-56 md:max-w-sm">
              <Search className="pointer-events-none absolute top-1/2 left-3.5 size-4 -translate-y-1/2 text-muted" aria-hidden="true" />
              <label htmlFor="tags-filter" className="sr-only">
                Find a tag
              </label>
              <input
                id="tags-filter"
                className={cn(inputClass, "h-10 pl-10")}
                type="search"
                placeholder="Find a tag"
                value={filter}
                maxLength={100}
                onChange={(e) => setFilter(e.target.value)}
              />
            </div>
            <Segmented
              label="Order"
              value={sort}
              onChange={setSort}
              options={[
                { value: "name", label: "A to Z" },
                { value: "count", label: "Most used" },
              ]}
            />
          </div>
        )}
      </PageHeader>
      <div className="mx-auto max-w-[96rem] px-4 pt-3 pb-10 md:px-8">
        {error && <InlineError>{error}</InlineError>}
        {!error && tags === null && <Loading>Loading your tags…</Loading>}
        {tags !== null && tags.length === 0 && (
          <Empty icon={TagIcon} title="No tags yet">
            Select photos and choose Tag, or open a photo&rsquo;s info and add a tag. Keywords stored in your photos are imported as tags.
          </Empty>
        )}
        {tags !== null && tags.length > 0 && shown.length === 0 && <p className="py-16 text-center text-sm text-muted">No tag has “{filter.trim()}” in its name.</p>}
        {shown.length > 0 && (
          <ul className="grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-3" aria-label="Tags">
            {shown.map((t, i) => (
              <Rise key={t.id} index={i}>
                <Link
                  to={`/tags/${t.id}`}
                  className="flex min-h-14 items-center gap-3 rounded-card bg-surface px-4 text-fg no-underline transition-colors duration-150 hover:bg-line/70 active:bg-line/70"
                >
                  <TagIcon className="size-4 shrink-0 text-muted" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate text-[0.9375rem] font-medium [overflow-wrap:anywhere]">{t.name}</span>
                  <span className="shrink-0 text-[0.8125rem] text-muted">{tagPhotos(t.count)}</span>
                </Link>
              </Rise>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}
