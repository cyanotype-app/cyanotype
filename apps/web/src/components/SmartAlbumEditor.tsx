import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { api, type Facets, type SmartPreview, type SmartRule, type SmartRules } from "../api";
import { MAX_RULES, NO_TEXT_QUERY_NOTE, RULE_FIELDS, blankRule, cleanRules, rulesProblem, smartCount, type RuleField } from "../smartText";
import { TagInput } from "./TagInput";

interface Props {
  title: string;
  /** The words of the primary button ("Create smart album", "Save"). */
  submitLabel: string;
  initialTitle?: string;
  initialRules: SmartRules;
  busy?: boolean;
  error?: string | null;
  onSubmit: (title: string, rules: SmartRules) => void;
  onCancel: () => void;
  /** Where the preview comes from (the tests pass fakes). */
  preview?: (rules: SmartRules, signal: AbortSignal) => Promise<SmartPreview>;
  /** The library's cameras, lenses and places, for suggestions. */
  loadFacets?: () => Promise<Facets>;
}

const defaultPreview = (rules: SmartRules, signal: AbortSignal) => api.previewSmartAlbum(rules, signal);

/**
 * The editor of a smart album: a title, "match all / any" over a list of rules, whether archived photos are in, and a live preview
 * of how many photos the rules match right now. A text search is not offered as a rule, and the editor says why.
 */
export function SmartAlbumEditor({
  title: heading,
  submitLabel,
  initialTitle = "",
  initialRules,
  busy = false,
  error = null,
  onSubmit,
  onCancel,
  preview = defaultPreview,
  loadFacets = api.searchFacets,
}: Props) {
  const id = useId();
  const [title, setTitle] = useState(initialTitle);
  const [rules, setRules] = useState<SmartRules>(initialRules);
  const [facets, setFacets] = useState<Facets | null>(null);
  const [shown, setShown] = useState<SmartPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [add, setAdd] = useState<RuleField>("place");
  const ref = useRef<HTMLFormElement>(null);

  useEffect(() => {
    let live = true;
    loadFacets().then(
      (f) => live && setFacets(f),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [loadFacets]);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("input")?.focus();
    return () => prev?.focus?.();
  }, []);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  const problem = rulesProblem(rules);
  const cleaned = cleanRules(rules);
  const key = JSON.stringify(cleaned);
  // The preview follows the rules a moment after the last change; an answer for rules that are gone is dropped.
  useEffect(() => {
    if (problem) return;
    const ctl = new AbortController();
    const t = setTimeout(() => {
      preview(JSON.parse(key) as SmartRules, ctl.signal).then(
        (p) => {
          setShown(p);
          setPreviewError(null);
        },
        (e: unknown) => {
          if (!ctl.signal.aborted) setPreviewError(e instanceof Error ? e.message : "Could not preview the rules.");
        },
      );
    }, 400);
    return () => {
      clearTimeout(t);
      ctl.abort();
    };
  }, [key, problem, preview]);

  const setRule = (i: number, r: SmartRule) => setRules((x) => ({ ...x, rules: x.rules.map((o, j) => (j === i ? r : o)) }));
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (title.trim() && !problem) onSubmit(title.trim(), cleaned);
  };

  return (
    <div className="modal-backdrop">
      <form className="modal card smart-editor" role="dialog" aria-modal="true" aria-labelledby={`${id}-h`} ref={ref} onSubmit={submit}>
        <h2 id={`${id}-h`}>{heading}</h2>
        <label className="smart-field" htmlFor={`${id}-title`}>
          <span>Title</span>
          <input id={`${id}-title`} className="text-input" value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} disabled={busy} />
        </label>
        <div className="smart-field">
          <span id={`${id}-match`}>Photos that match</span>
          <div className="sheet-segments" role="radiogroup" aria-labelledby={`${id}-match`}>
            {(["all", "any"] as const).map((m) => (
              <label key={m} className="sheet-segment">
                <input type="radio" name={`${id}-m`} checked={rules.match === m} onChange={() => setRules({ ...rules, match: m })} disabled={busy} />
                <span>{m === "all" ? "All of the rules" : "Any of the rules"}</span>
              </label>
            ))}
          </div>
        </div>
        <ul className="smart-rules" aria-label="Rules">
          {rules.rules.map((r, i) => (
            <li key={i} className="smart-rule">
              <RuleRow rule={r} facets={facets} onChange={(n) => setRule(i, n)} disabled={busy} index={i} />
              <button
                type="button"
                className="btn btn-quiet smart-rule-x"
                aria-label={`Remove rule ${i + 1}`}
                disabled={busy}
                onClick={() => setRules({ ...rules, rules: rules.rules.filter((_, j) => j !== i) })}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        <div className="smart-add">
          <label htmlFor={`${id}-add`} className="sr-only">
            Kind of rule to add
          </label>
          <select id={`${id}-add`} className="text-input" value={add} onChange={(e) => setAdd(e.target.value as RuleField)} disabled={busy}>
            {RULE_FIELDS.map((f) => (
              <option key={f.field} value={f.field}>
                {f.label}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn btn-quiet"
            disabled={busy || rules.rules.length >= MAX_RULES}
            onClick={() => setRules({ ...rules, rules: [...rules.rules, blankRule(add)] })}
          >
            Add rule
          </button>
        </div>
        <p className="muted small smart-note">{NO_TEXT_QUERY_NOTE}</p>
        <label className="sheet-check">
          <input
            type="checkbox"
            checked={rules.archived === "include"}
            onChange={(e) => setRules({ ...rules, archived: e.target.checked ? "include" : "exclude" })}
            disabled={busy}
          />
          <span>Include archived photos</span>
        </label>
        <div className="smart-preview" aria-live="polite">
          {problem ? (
            <p className="muted small">{problem}</p>
          ) : previewError ? (
            <p role="alert" className="form-error small">
              {previewError}
            </p>
          ) : shown ? (
            <>
              <p className="small">
                <strong>{smartCount(shown.count, shown.capped)}</strong> match now. The album always shows what matches when you open it.
              </p>
              <ul className="smart-thumbs" aria-label="A few of the matches">
                {shown.items.map((it) => (
                  <li key={it.id}>{it.thumb_url ? <img src={it.thumb_url} alt="" loading="lazy" /> : <span className="smart-thumb-blank" />}</li>
                ))}
              </ul>
            </>
          ) : (
            <p className="muted small">Counting…</p>
          )}
        </div>
        {error && (
          <p role="alert" className="form-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn btn-primary" disabled={busy || !title.trim() || problem !== null}>
            {submitLabel}
          </button>
        </div>
      </form>
    </div>
  );
}

function RuleRow({ rule, facets, onChange, disabled, index }: { rule: SmartRule; facets: Facets | null; onChange: (r: SmartRule) => void; disabled: boolean; index: number }) {
  const id = useId();
  const n = index + 1;
  const label = RULE_FIELDS.find((f) => f.field === rule.field)?.label ?? rule.field;
  const listFor = (f: "place" | "camera" | "lens") =>
    f === "camera" ? facets?.cameras.map((c) => c.value) : f === "lens" ? facets?.lenses?.map((c) => c.value) : facets?.places.map((p) => p.value);
  switch (rule.field) {
    case "date":
      return (
        <div className="smart-rule-body">
          <span className="smart-rule-label">{label}</span>
          <label className="smart-mini">
            <span>From</span>
            <input className="text-input" type="date" value={rule.from ?? ""} max={rule.to || undefined} onChange={(e) => onChange({ ...rule, from: e.target.value })} disabled={disabled} aria-label={`Rule ${n}: from`} />
          </label>
          <label className="smart-mini">
            <span>To</span>
            <input className="text-input" type="date" value={rule.to ?? ""} min={rule.from || undefined} onChange={(e) => onChange({ ...rule, to: e.target.value })} disabled={disabled} aria-label={`Rule ${n}: to`} />
          </label>
        </div>
      );
    case "place":
    case "camera":
    case "lens": {
      const options = listFor(rule.field) ?? [];
      return (
        <div className="smart-rule-body">
          <label className="smart-rule-label" htmlFor={`${id}-v`}>
            {label}
          </label>
          <input
            id={`${id}-v`}
            className="text-input"
            value={rule.value}
            maxLength={200}
            list={options.length ? `${id}-l` : undefined}
            onChange={(e) => onChange({ ...rule, value: e.target.value })}
            disabled={disabled}
          />
          {options.length > 0 && (
            <datalist id={`${id}-l`}>
              {options.map((o) => (
                <option key={o} value={o} />
              ))}
            </datalist>
          )}
        </div>
      );
    }
    case "type":
      return (
        <div className="smart-rule-body">
          <label className="smart-rule-label" htmlFor={`${id}-v`}>
            {label}
          </label>
          <select id={`${id}-v`} className="text-input" value={rule.value} onChange={(e) => onChange({ ...rule, value: e.target.value })} disabled={disabled}>
            {[
              ["jpeg", "JPEG"],
              ["heic", "HEIC"],
              ["raw", "RAW"],
              ["other", "Other"],
            ].map(([v, l]) => (
              <option key={v} value={v}>
                {l}
              </option>
            ))}
          </select>
        </div>
      );
    case "favorite":
      return (
        <div className="smart-rule-body">
          <span className="smart-rule-label">{label}</span>
          <span className="muted">Photos with a heart</span>
        </div>
      );
    case "rating":
      return (
        <div className="smart-rule-body">
          <label className="smart-rule-label" htmlFor={`${id}-v`}>
            {label}
          </label>
          <select id={`${id}-v`} className="text-input" value={rule.min} onChange={(e) => onChange({ ...rule, min: Number(e.target.value) })} disabled={disabled}>
            {[1, 2, 3, 4, 5].map((s) => (
              <option key={s} value={s}>
                {s === 5 ? "5 stars" : `${s} ${s === 1 ? "star" : "stars"} & up`}
              </option>
            ))}
          </select>
        </div>
      );
    case "tags":
      return (
        <div className="smart-rule-body smart-rule-tags">
          <TagInput label={`${label} (rule ${n})`} existingOnly value={rule.names} onChange={(names) => onChange({ ...rule, names: [...names] })} disabled={disabled} placeholder="Choose tags" />
          <select className="text-input" aria-label={`Rule ${n}: match`} value={rule.mode} onChange={(e) => onChange({ ...rule, mode: e.target.value === "any" ? "any" : "all" })} disabled={disabled}>
            <option value="all">All of these tags</option>
            <option value="any">Any of these tags</option>
          </select>
        </div>
      );
    case "has_location":
    case "has_text":
      return (
        <div className="smart-rule-body">
          <label className="smart-rule-label" htmlFor={`${id}-v`}>
            {label}
          </label>
          <select id={`${id}-v`} className="text-input" value={String(rule.value)} onChange={(e) => onChange({ ...rule, value: e.target.value === "true" })} disabled={disabled}>
            <option value="true">{rule.field === "has_text" ? "Has text" : "Has a location"}</option>
            <option value="false">{rule.field === "has_text" ? "Has no text" : "Has no location"}</option>
          </select>
        </div>
      );
    case "caption":
      return (
        <div className="smart-rule-body">
          <label className="smart-rule-label" htmlFor={`${id}-v`}>
            {label}
          </label>
          <input id={`${id}-v`} className="text-input" value={rule.text} maxLength={200} onChange={(e) => onChange({ ...rule, text: e.target.value })} disabled={disabled} />
        </div>
      );
  }
}
