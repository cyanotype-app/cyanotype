import { Clock, Search, X } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { cn } from "../ui/cn";
import { paramsFromUrl, clearRecent, loadRecent, pushRecent, suggestionsFor, urlFromParams } from "../searchText";

/** Navigation state that asks the Search page to focus its field (the shortcut and the tab bar set it). */
export const FOCUS_SEARCH = { focusSearch: true } as const;

/**
 * The search field of the Search page: large and always visible, with `children` (the Filters button) beside it.
 * Submitting goes to `/search?q=...`, keeping the filters already chosen there. While something is typed, the recent
 * searches that continue it drop down under the field; `recentsInPage` says the page itself lists them (the empty
 * search), so an empty field does not drop the same list down on top of it.
 *
 * The field takes focus when the page opens with nothing searched yet (a phone's tab, a desktop visit) and whenever
 * the page is asked to (`FOCUS_SEARCH` in the navigation state, from the keyboard shortcut).
 */
export function SearchBar({ recentsInPage, children }: { recentsInPage: boolean; children?: ReactNode }) {
  const navigate = useNavigate();
  const loc = useLocation();
  const current = useMemo(() => paramsFromUrl(new URLSearchParams(loc.search)), [loc.search]);
  const [typed, setTyped] = useState(current.q ?? "");
  const [focused, setFocused] = useState(false);
  const [recent, setRecent] = useState<string[]>(() => loadRecent());
  const input = useRef<HTMLInputElement>(null);
  // The list closes a moment after the field loses focus (so a click on a suggestion still lands); a focus in between
  // must cancel that, or a quick refocus would find the list gone.
  const blurTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(blurTimer.current), []);
  const listId = useId();
  const inputId = `${listId}-input`;

  // The page's own query (back button, a chip, a recent search) is what the field shows.
  const urlQuery = current.q ?? "";
  useEffect(() => setTyped(urlQuery), [urlQuery]);

  // Focus on arrival at an empty search, and when asked to (a new history entry carries the request).
  const nothingSearched = Object.keys(current).length === 0;
  const asked = (loc.state as { focusSearch?: boolean } | null)?.focusSearch === true;
  const arrived = useRef(true);
  useEffect(() => {
    const first = arrived.current;
    arrived.current = false;
    if (asked || (first && nothingSearched)) {
      input.current?.focus();
      if (asked) input.current?.select();
    }
    // Only a new visit or a new request: typing and filtering change the address but must not steal focus back.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loc.key]);

  function go(q: string) {
    const text = q.trim();
    if (text) setRecent(pushRecent(text));
    const params = { ...current, q: text || undefined };
    if (!text) delete params.q;
    const qs = urlFromParams(params);
    navigate(qs ? `/search?${qs}` : "/search");
    setFocused(false);
    input.current?.blur();
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    go(typed);
  }

  const suggestions = suggestionsFor(typed, recent);
  const showList = focused && suggestions.length > 0 && !(typed.trim() === "" && recentsInPage);

  return (
    <div className="flex items-center gap-2">
      <form className="relative min-w-0 flex-1" role="search" onSubmit={submit}>
        <label htmlFor={inputId} className="sr-only">
          Search photos
        </label>
        <div className="flex h-12 items-center gap-2.5 rounded-full bg-surface px-4 transition-shadow focus-within:ring-2 focus-within:ring-accent">
          <Search className="size-5 shrink-0 text-muted" aria-hidden="true" />
          <input
            id={inputId}
            ref={input}
            className="h-full min-w-0 flex-1 appearance-none bg-transparent text-base text-fg outline-none placeholder:text-muted [&::-webkit-search-cancel-button]:appearance-none"
            type="search"
            enterKeyHint="search"
            autoComplete="off"
            spellCheck={false}
            maxLength={200}
            placeholder="Photos, places, people, text"
            value={typed}
            role="combobox"
            aria-expanded={showList}
            aria-controls={listId}
            aria-autocomplete="list"
            onChange={(e) => setTyped(e.target.value)}
            onFocus={() => {
              clearTimeout(blurTimer.current);
              setRecent(loadRecent()); // another tab may have searched since this one loaded
              setFocused(true);
            }}
            onBlur={() => {
              clearTimeout(blurTimer.current);
              blurTimer.current = setTimeout(() => setFocused(false), 120);
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                if (typed !== urlQuery) setTyped(urlQuery);
                else input.current?.blur();
              }
            }}
          />
          {typed && (
            <button
              type="button"
              aria-label="Clear search text"
              className="-mr-2 inline-flex size-8 shrink-0 items-center justify-center rounded-full text-muted hover:bg-line/60 hover:text-fg"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                setTyped("");
                input.current?.focus();
              }}
            >
              <X className="size-4" aria-hidden="true" />
            </button>
          )}
        </div>
        {/* (always in the document: the input's aria-controls points at it) */}
        <ul
          id={listId}
          role="listbox"
          aria-label="Recent searches"
          className={cn(
            "absolute inset-x-0 top-full z-20 mt-2 overflow-hidden rounded-card bg-card py-1 shadow-float ring-1 ring-line",
            !showList && "hidden",
          )}
        >
          {showList && (
            <>
              {suggestions.map((r) => (
                <li key={r} role="option" aria-selected="false">
                  <button
                    type="button"
                    className="flex h-11 w-full items-center gap-3 px-4 text-left text-[0.9375rem] hover:bg-surface"
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => {
                      setTyped(r);
                      go(r);
                    }}
                  >
                    <Clock className="size-4 shrink-0 text-muted" aria-hidden="true" />
                    <span className="min-w-0 flex-1 truncate">{r}</span>
                  </button>
                </li>
              ))}
              <li role="presentation" className="border-t border-line">
                <button
                  type="button"
                  className="h-11 w-full px-4 text-left text-sm text-muted hover:bg-surface hover:text-fg"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    clearRecent();
                    setRecent([]);
                  }}
                >
                  Clear recent searches
                </button>
              </li>
            </>
          )}
        </ul>
      </form>
      {children}
    </div>
  );
}

/**
 * Ctrl+K (⌘K on a Mac) from anywhere in the app: go to Search with the field focused and its text selected. A plain
 * shortcut rather than a command palette: the Search page already is the palette (recent searches, people, places,
 * dates and filters on one screen), so a second popup that answers the same question would only be a second place to
 * keep in step. Ctrl+K is not a typing key, so it works inside text fields too. On the Search page it only moves focus
 * (the results stay); elsewhere it opens an empty search.
 */
export function SearchShortcut() {
  const navigate = useNavigate();
  const loc = useLocation();
  const here = loc.pathname === "/search" ? loc.pathname + loc.search : null;
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        navigate(here ?? "/search", { state: FOCUS_SEARCH, replace: here !== null });
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [navigate, here]);
  return null;
}
