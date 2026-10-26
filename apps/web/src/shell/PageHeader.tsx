import { ChevronLeft } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { cn } from "../ui/cn";
import { AccountMenu } from "./AppShell";

/**
 * A page's title bar: sticky and frosted, with a hairline once the page has scrolled under it. The title is large
 * (iOS style) and the page's actions sit at its right; on a phone the account button follows them, since a phone has
 * no sidebar. `children` is an optional second row (tabs, filters, a search field).
 */
export function PageHeader({
  title,
  subtitle,
  back,
  actions,
  children,
  className,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  /** Where the back chevron goes (a person's page goes back to People). No chevron when absent. */
  back?: { to: string; label: string };
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  const scrolled = useScrolled();
  return (
    <header
      className={cn(
        "glass sticky top-0 z-[5] border-b px-4 pt-[max(0.75rem,env(safe-area-inset-top,0px))] pb-2.5 transition-[border-color] duration-200 md:px-8 md:pt-5",
        scrolled ? "border-line" : "border-transparent",
        className,
      )}
    >
      {back && (
        <Link
          to={back.to}
          className="-ml-1.5 mb-1 inline-flex h-8 items-center gap-0.5 rounded-full pr-2 text-[0.9375rem] text-accent no-underline hover:bg-surface"
        >
          <ChevronLeft className="size-5" aria-hidden="true" />
          {back.label}
        </Link>
      )}
      <div className="flex min-h-10 items-center gap-2">
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-[1.75rem] leading-tight font-bold tracking-[-0.02em] md:text-[1.625rem]">{title}</h1>
          {subtitle && <p className="truncate text-sm text-muted">{subtitle}</p>}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-1">{actions}</div>}
        <div className="shrink-0 md:hidden">
          <AccountMenu />
        </div>
      </div>
      {children && <div className="mt-2.5">{children}</div>}
    </header>
  );
}

function useScrolled(): boolean {
  const [scrolled, setScrolled] = useState(false);
  useEffect(() => {
    const on = () => setScrolled(window.scrollY > 4);
    on();
    window.addEventListener("scroll", on, { passive: true });
    return () => window.removeEventListener("scroll", on);
  }, []);
  return scrolled;
}
