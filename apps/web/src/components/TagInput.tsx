import { useEffect, useId, useRef, useState } from "react";
import { api, type Tag } from "../api";
import { cn } from "../ui/cn";
import { TagChip } from "./TagChip";
import { MAX_TAGS_AT_ONCE, nameProblem, suggest, tagPhotos, tidy, withName } from "../tagText";

interface Props {
  /** The names chosen so far. */
  value: readonly string[];
  onChange: (next: readonly string[]) => void;
  /** Visible or screen-reader label of the field ("Add tags"). */
  label: string;
  placeholder?: string;
  /** Only names that are tags already can be chosen (the "remove" field, the filter). Typing a new name offers nothing. */
  existingOnly?: boolean;
  disabled?: boolean;
  /** Where autocomplete comes from (tests pass fakes). The default asks the server for names containing the text. */
  load?: (typed: string) => Promise<Tag[]>;
  autoFocus?: boolean;
}

const defaultLoad = (typed: string) => api.listTags({ q: typed || undefined, sort: "count", limit: 12 }).then((r) => r.items);

/**
 * Tags as chips with an autocompleting text field. Enter or a comma adds what is typed (or the highlighted suggestion), Backspace
 * on an empty field takes the last chip back, the arrow keys move through the suggestions. A name is checked as it is added
 * (no commas, at most 100 characters), so a bad one is never sent.
 */
export function TagInput({ value, onChange, label, placeholder, existingOnly = false, disabled = false, load = defaultLoad, autoFocus = false }: Props) {
  const id = useId();
  const [typed, setTyped] = useState("");
  const [options, setOptions] = useState<Tag[]>([]);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [problem, setProblem] = useState<string | null>(null);
  const [known, setKnown] = useState<Map<string, Tag>>(new Map());
  const input = useRef<HTMLInputElement>(null);
  // The list closes a moment after blur (so a click on an option still lands). The timer must not outlive the component.
  const closing = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(closing.current), []);

  // Autocomplete: what the server says for the text typed so far, a moment after the last key.
  useEffect(() => {
    if (disabled) return;
    let live = true;
    const t = setTimeout(() => {
      load(tidy(typed)).then(
        (tags) => {
          if (!live) return;
          setOptions(tags);
          setKnown((m) => {
            const next = new Map(m);
            for (const x of tags) next.set(x.name.toLowerCase(), x);
            return next;
          });
        },
        () => live && setOptions([]),
      );
    }, 120);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [typed, load, disabled]);

  const shown = suggest(typed, options, value);
  const listId = `${id}-list`;

  async function add(raw: string) {
    const name = tidy(raw);
    if (!name) return;
    const bad = nameProblem(name);
    if (bad) return setProblem(bad);
    if (value.length >= MAX_TAGS_AT_ONCE) return setProblem(`At most ${MAX_TAGS_AT_ONCE} tags at a time.`);
    let spelled = known.get(name.toLowerCase())?.name;
    if (existingOnly && !spelled) {
      // Typed faster than the suggestions arrived: ask for exactly this name before refusing it.
      const found = await load(name).catch(() => [] as Tag[]);
      spelled = found.find((t) => t.name.toLowerCase() === name.toLowerCase())?.name;
      if (!spelled) return setProblem(`“${name}” is not one of your tags.`);
    }
    // The spelling the library already uses wins over what was typed.
    onChange(withName(value, spelled ?? name));
    setTyped("");
    setProblem(null);
    setActive(-1);
  }

  return (
    <div className="relative">
      <label htmlFor={`${id}-field`} className="mb-1.5 block text-sm font-medium">
        {label}
      </label>
      <div
        className="flex min-h-11 flex-wrap items-center gap-1.5 rounded-card border border-line bg-bg px-2.5 py-1.5 focus-within:border-accent focus-within:ring-2 focus-within:ring-accent/25"
        onClick={() => input.current?.focus()}
      >
        {value.map((n) => (
          <TagChip key={n.toLowerCase()} removeLabel={`Remove ${n}`} disabled={disabled} onRemove={() => onChange(value.filter((x) => x !== n))}>
            {n}
          </TagChip>
        ))}
        <input
          id={`${id}-field`}
          ref={input}
          className="min-w-[8ch] flex-1 bg-transparent py-1 text-base outline-none placeholder:text-muted focus-visible:outline-none"
          role="combobox"
          aria-expanded={open && shown.length > 0}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={active >= 0 ? `${id}-opt-${active}` : undefined}
          autoComplete="off"
          autoFocus={autoFocus}
          disabled={disabled}
          placeholder={value.length === 0 ? placeholder : undefined}
          value={typed}
          maxLength={160}
          onFocus={() => {
            clearTimeout(closing.current);
            setOpen(true);
          }}
          onBlur={() => {
            clearTimeout(closing.current);
            closing.current = setTimeout(() => setOpen(false), 120);
          }}
          onChange={(e) => {
            const v = e.target.value;
            setProblem(null);
            setActive(-1);
            // A comma ends a name (the commas typed or pasted split a list).
            if (v.includes(",")) {
              const parts = v.split(",");
              for (const p of parts.slice(0, -1)) void add(p);
              setTyped(parts.at(-1) ?? "");
            } else setTyped(v);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === "Tab") {
              if (e.key === "Tab" && !typed.trim()) return;
              const pick = active >= 0 ? shown[active] : undefined;
              if (pick || typed.trim()) {
                e.preventDefault();
                e.stopPropagation();
                void add(pick ? pick.name : typed);
              }
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              setOpen(true);
              setActive((a) => Math.min(shown.length - 1, a + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => Math.max(-1, a - 1));
            } else if (e.key === "Backspace" && typed === "" && value.length > 0) {
              onChange(value.slice(0, -1));
            } else if (e.key === "Escape" && open && shown.length > 0) {
              // The first Escape puts the suggestions away; a second one reaches the dialog.
              e.stopPropagation();
              setOpen(false);
            }
          }}
        />
      </div>
      {open && shown.length > 0 && (
        <ul
          className="absolute inset-x-0 top-full z-10 mt-1 max-h-56 overflow-y-auto rounded-card border border-line bg-card p-1 shadow-float"
          id={listId}
          role="listbox"
          aria-label={`${label}: suggestions`}
        >
          {shown.map((t, i) => (
            <li
              key={t.id}
              id={`${id}-opt-${i}`}
              role="option"
              aria-selected={i === active}
              className={cn("flex min-h-10 cursor-pointer items-center justify-between gap-3 rounded-lg px-2.5 text-[0.9375rem]", i === active && "bg-surface")}
              // Mouse down, not click: the field loses focus first and would close the list.
              onMouseDown={(e) => {
                e.preventDefault();
                void add(t.name);
              }}
            >
              <span className="min-w-0 [overflow-wrap:anywhere]">{t.name}</span>
              <span className="shrink-0 text-xs text-muted">{tagPhotos(t.count)}</span>
            </li>
          ))}
        </ul>
      )}
      {problem && (
        <p role="alert" className="mt-1.5 text-sm text-danger">
          {problem}
        </p>
      )}
    </div>
  );
}
