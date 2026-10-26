import { ChevronRight } from "lucide-react";
import { Link } from "react-router-dom";
import { LIBRARY, UTILITIES, type NavItem } from "../shell/nav";
import { PageHeader } from "../shell/PageHeader";
import { UploadMenu } from "../shell/AppShell";
import { usePeopleEnabled } from "../usePeople";

/** The home page: the library's collections (memories, albums, places, people), then its utilities. */
export function Collections() {
  const people = usePeopleEnabled() === true;
  const library = LIBRARY.filter((i) => !i.people || people);
  return (
    <>
      <PageHeader title="Collections" actions={<span className="md:hidden"><UploadMenu compact /></span>} />
      <div className="mx-auto flex max-w-5xl flex-col gap-8 px-4 pt-2 pb-10 md:px-8">
        <LinkList title="Library" items={library} />
        <LinkList title="Utilities" items={UTILITIES} />
      </div>
    </>
  );
}

function LinkList({ title, items }: { title: string; items: readonly NavItem[] }) {
  return (
    <section aria-labelledby={`h-${title}`}>
      <h2 id={`h-${title}`} className="mb-2 text-[1.0625rem] font-semibold tracking-tight">
        {title}
      </h2>
      <ul className="divide-y divide-line overflow-hidden rounded-card bg-card ring-1 ring-line">
        {items.map((i) => {
          const Icon = i.icon;
          return (
            <li key={i.to}>
              <Link
                to={i.to}
                className="flex h-12 items-center gap-3 px-4 text-[0.9375rem] text-fg no-underline transition-colors hover:bg-surface active:bg-surface"
              >
                <Icon className="size-5 text-accent" aria-hidden="true" />
                <span className="flex-1">{i.label}</span>
                <ChevronRight className="size-4 text-muted" aria-hidden="true" />
              </Link>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
