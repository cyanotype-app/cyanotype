import { useEffect, useId, useMemo, useRef, useState, type FormEvent } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { paramsFromUrl, clearRecent, loadRecent, pushRecent, suggestionsFor, urlFromParams } from "../searchText";

/**
 * The search field in the library header. On a wide screen it is a field next to the tabs; on a phone it is an icon at
 * the end of the tab row that expands into a full-width field (taking the row over, with a back button) so the tabs
 * and the field never have to share 320 px. Submitting goes to `/search?q=...`, keeping the filters already chosen
 * there; recent searches (this browser only) drop down under the field.
 */
export function SearchBar({ onOpenChange }: { onOpenChange?: (open: boolean) => void }) {
  const navigate = useNavigate();
  const loc = useLocation();
  const onSearchPage = loc.pathname === "/search";
  const current = useMemo(() => paramsFromUrl(new URLSearchParams(loc.search)), [loc.search]);
  const [typed, setTyped] = useState(onSearchPage ? (current.q ?? "") : "");
  const [open, setOpen] = useState(onSearchPage); // phone: field shown instead of the tabs
  const [focused, setFocused] = useState(false);
  const [recent, setRecent] = useState<string[]>(() => loadRecent());
  const input = useRef<HTMLInputElement>(null);
  // The list closes a moment after the field loses focus (so a click on a suggestion still lands); a focus in between
  // must cancel that, or a quick refocus would find the list gone.
  const blurTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(blurTimer.current), []);
  const listId = useId();

  // The page's own query (back button, a chip, a recent search) is what the field shows.
  const urlQuery = onSearchPage ? (current.q ?? "") : "";
  useEffect(() => setTyped(urlQuery), [urlQuery]);
  useEffect(() => {
    if (onSearchPage) setOpen(true);
  }, [onSearchPage]);
  useEffect(() => onOpenChange?.(open), [open, onOpenChange]);

  function go(q: string) {
    const text = q.trim();
    if (text) setRecent(pushRecent(text));
    const keep = onSearchPage ? current : {};
    const params = { ...keep, q: text || undefined };
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

  function expand() {
    setOpen(true);
    // After the field is rendered visible.
    requestAnimationFrame(() => input.current?.focus());
  }

  function collapse() {
    setOpen(false);
    setFocused(false);
    // Leaving an empty search page by closing the field returns to the library.
    if (onSearchPage && !current.q && Object.keys(current).length === 0) navigate("/photos");
  }

  const suggestions = suggestionsFor(typed, recent);
  const showList = focused && suggestions.length > 0;

  return (
    <div className="search" data-open={open}>
      <button
        type="button"
        className="search-toggle"
        aria-label="Search photos"
        aria-expanded={open}
        onClick={expand}
      >
        <SearchIcon />
      </button>
      <form className="search-form" role="search" onSubmit={submit}>
        <button type="button" className="search-back" aria-label="Close search" onClick={collapse}>
          <BackIcon />
        </button>
        <label htmlFor={`${listId}-input`} className="sr-only">
          Search photos
        </label>
        <div className="search-field">
          <SearchIcon />
          <input
            id={`${listId}-input`}
            ref={input}
            className="search-input"
            type="search"
            enterKeyHint="search"
            autoComplete="off"
            spellCheck={false}
            maxLength={200}
            placeholder="Search: dog on the beach"
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
                if (typed) setTyped(urlQuery);
                else input.current?.blur();
              }
            }}
          />
        </div>
        {showList && (
          <ul className="search-recent" id={listId} role="listbox" aria-label="Recent searches">
            {suggestions.map((r) => (
              <li key={r} role="option" aria-selected="false">
                <button
                  type="button"
                  className="search-recent-item"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    setTyped(r);
                    go(r);
                  }}
                >
                  <ClockIcon />
                  <span>{r}</span>
                </button>
              </li>
            ))}
            <li role="presentation">
              <button
                type="button"
                className="search-recent-clear"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  clearRecent();
                  setRecent([]);
                }}
              >
                Clear recent searches
              </button>
            </li>
          </ul>
        )}
      </form>
    </div>
  );
}

function SearchIcon() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="m15.5 15.5 5 5" />
    </svg>
  );
}

function BackIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="m15 5-7 7 7 7" />
    </svg>
  );
}

function ClockIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7.5V12l3 2" />
    </svg>
  );
}
