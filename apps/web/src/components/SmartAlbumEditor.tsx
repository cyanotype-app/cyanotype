import { Plus, X } from "lucide-react";
import { useEffect, useId, useState, type FormEvent, type ReactNode } from "react";
import { api, type Facets, type Person, type SmartPreview, type SmartRule, type SmartRules } from "../api";
import { MAX_FILTER_PEOPLE, NO_NAMED_PEOPLE, filterablePeople } from "../peopleText";
import { MAX_RULES, NO_TEXT_QUERY_NOTE, RULE_FIELDS, blankRule, cleanRules, ruleFieldsFor, rulesProblem, smartCount, type RuleField } from "../smartText";
import { usePeopleEnabled } from "../usePeople";
import { Button } from "../ui/Button";
import { Segmented } from "../ui/Controls";
import { cn } from "../ui/cn";
import { PeoplePicker } from "./PeoplePicker";
import { CheckRow, Field, FormError, inputClass, Modal, Note, SelectBox, TextInput } from "./sheetForm";
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
  /** The owner's people, for the People rule (asked only while People is on). */
  loadPeople?: () => Promise<Person[]>;
}

const defaultPreview = (rules: SmartRules, signal: AbortSignal) => api.previewSmartAlbum(rules, signal);
const defaultPeople = () => api.listPeople().then((r) => r.items);

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
  loadPeople = defaultPeople,
}: Props) {
  const id = useId();
  const [title, setTitle] = useState(initialTitle);
  const [rules, setRules] = useState<SmartRules>(initialRules);
  const [facets, setFacets] = useState<Facets | null>(null);
  const peopleOn = usePeopleEnabled() === true;
  const [people, setPeople] = useState<Person[] | null>(null);
  const [shown, setShown] = useState<SmartPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [add, setAdd] = useState<RuleField>("place");

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
    if (!peopleOn) return;
    let live = true;
    loadPeople().then(
      (p) => live && setPeople(filterablePeople(p)),
      () => live && setPeople([]),
    );
    return () => {
      live = false;
    };
  }, [peopleOn, loadPeople]);

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
    <Modal
      onClose={onCancel}
      title={heading}
      size="lg"
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form={id} disabled={busy || !title.trim() || problem !== null}>
            {submitLabel}
          </Button>
        </>
      }
    >
      <form id={id} className="flex flex-col gap-5" onSubmit={submit}>
        <Field label="Title" htmlFor={`${id}-title`}>
          <TextInput id={`${id}-title`} data-autofocus="" value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} disabled={busy} />
        </Field>
        <div className="flex flex-col gap-1.5">
          <span className="text-[0.8125rem] font-medium text-muted">Photos that match</span>
          <Segmented
            label="Photos that match"
            value={rules.match}
            onChange={(m) => setRules({ ...rules, match: m })}
            options={[
              { value: "all", label: "All of the rules" },
              { value: "any", label: "Any of the rules" },
            ]}
            className="self-start"
          />
        </div>
        <ul className="flex flex-col gap-2.5" aria-label="Rules">
          {rules.rules.map((r, i) => (
            <li key={i} className="flex items-start gap-2 rounded-card bg-surface p-3">
              <div className="min-w-0 flex-1">
                <RuleRow rule={r} facets={facets} people={people} onChange={(n) => setRule(i, n)} disabled={busy} index={i} />
              </div>
              <Button
                variant="quiet"
                size="icon-sm"
                aria-label={`Remove rule ${i + 1}`}
                disabled={busy}
                onClick={() => setRules({ ...rules, rules: rules.rules.filter((_, j) => j !== i) })}
              >
                <X />
              </Button>
            </li>
          ))}
        </ul>
        <div className="flex items-center gap-2">
          <label htmlFor={`${id}-add`} className="sr-only">
            Kind of rule to add
          </label>
          <div className="min-w-0 flex-1">
            <SelectBox id={`${id}-add`} value={add} onChange={(e) => setAdd(e.target.value as RuleField)} disabled={busy}>
              {ruleFieldsFor(peopleOn).map((f) => (
                <option key={f.field} value={f.field}>
                  {f.label}
                </option>
              ))}
            </SelectBox>
          </div>
          <Button
            variant="secondary"
            disabled={busy || rules.rules.length >= MAX_RULES}
            onClick={() => setRules({ ...rules, rules: [...rules.rules, blankRule(add)] })}
          >
            <Plus />
            Add rule
          </Button>
        </div>
        <Note>{NO_TEXT_QUERY_NOTE}</Note>
        <CheckRow checked={rules.archived === "include"} onChange={(on) => setRules({ ...rules, archived: on ? "include" : "exclude" })} disabled={busy}>
          Include archived photos
        </CheckRow>
        <div className="flex flex-col gap-3 rounded-card bg-surface p-3.5" aria-live="polite">
          {problem ? (
            <Note>{problem}</Note>
          ) : previewError ? (
            <FormError>{previewError}</FormError>
          ) : shown ? (
            <>
              <p className="text-sm">
                <strong className="font-semibold">{smartCount(shown.count, shown.capped)}</strong> match now. The album always shows what matches when you open it.
              </p>
              <ul className="grid grid-cols-4 gap-1.5 sm:grid-cols-6" aria-label="A few of the matches">
                {shown.items.slice(0, 6).map((it) => (
                  <li key={it.id} className="aspect-square overflow-hidden rounded-tile bg-line">
                    {it.thumb_url && <img src={it.thumb_url} alt="" loading="lazy" className="size-full object-cover" />}
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <Note>Counting…</Note>
          )}
        </div>
        {error && <FormError>{error}</FormError>}
      </form>
    </Modal>
  );
}

function RuleRow({
  rule,
  facets,
  people,
  onChange,
  disabled,
  index,
}: {
  rule: SmartRule;
  facets: Facets | null;
  people: Person[] | null;
  onChange: (r: SmartRule) => void;
  disabled: boolean;
  index: number;
}) {
  const id = useId();
  const n = index + 1;
  const label = RULE_FIELDS.find((f) => f.field === rule.field)?.label ?? rule.field;
  const listFor = (f: "place" | "camera" | "lens") =>
    f === "camera" ? facets?.cameras.map((c) => c.value) : f === "lens" ? facets?.lenses?.map((c) => c.value) : facets?.places.map((p) => p.value);
  // Inside a grey rule card the fields sit on the sheet's colour.
  const field = "bg-card";
  switch (rule.field) {
    case "date":
      return (
        <Body>
          <span className="text-[0.8125rem] font-medium text-muted">{label}</span>
          <div className="grid grid-cols-2 gap-2">
            <label className="flex flex-col gap-1 text-[0.8125rem] text-muted">
              <span>From</span>
              <input className={cn(inputClass, field)} type="date" value={rule.from ?? ""} max={rule.to || undefined} onChange={(e) => onChange({ ...rule, from: e.target.value })} disabled={disabled} aria-label={`Rule ${n}: from`} />
            </label>
            <label className="flex flex-col gap-1 text-[0.8125rem] text-muted">
              <span>To</span>
              <input className={cn(inputClass, field)} type="date" value={rule.to ?? ""} min={rule.from || undefined} onChange={(e) => onChange({ ...rule, to: e.target.value })} disabled={disabled} aria-label={`Rule ${n}: to`} />
            </label>
          </div>
        </Body>
      );
    case "place":
    case "camera":
    case "lens": {
      const options = listFor(rule.field) ?? [];
      return (
        <Body>
          <Label htmlFor={`${id}-v`}>{label}</Label>
          <input
            id={`${id}-v`}
            className={cn(inputClass, field)}
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
        </Body>
      );
    }
    case "type":
      return (
        <Body>
          <Label htmlFor={`${id}-v`}>{label}</Label>
          <SelectBox id={`${id}-v`} className={field} value={rule.value} onChange={(e) => onChange({ ...rule, value: e.target.value })} disabled={disabled}>
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
          </SelectBox>
        </Body>
      );
    case "favorite":
      return (
        <Body>
          <span className="text-[0.8125rem] font-medium text-muted">{label}</span>
          <span className="text-[0.9375rem]">Photos with a heart</span>
        </Body>
      );
    case "rating":
      return (
        <Body>
          <Label htmlFor={`${id}-v`}>{label}</Label>
          <SelectBox id={`${id}-v`} className={field} value={rule.min} onChange={(e) => onChange({ ...rule, min: Number(e.target.value) })} disabled={disabled}>
            {[1, 2, 3, 4, 5].map((s) => (
              <option key={s} value={s}>
                {s === 5 ? "5 stars" : `${s} ${s === 1 ? "star" : "stars"} & up`}
              </option>
            ))}
          </SelectBox>
        </Body>
      );
    case "tags":
      return (
        <Body className="gap-2">
          <TagInput label={`${label} (rule ${n})`} existingOnly value={rule.names} onChange={(names) => onChange({ ...rule, names: [...names] })} disabled={disabled} placeholder="Choose tags" />
          <SelectBox className={field} aria-label={`Rule ${n}: match`} value={rule.mode} onChange={(e) => onChange({ ...rule, mode: e.target.value === "any" ? "any" : "all" })} disabled={disabled}>
            <option value="all">All of these tags</option>
            <option value="any">Any of these tags</option>
          </SelectBox>
        </Body>
      );
    case "has_location":
    case "has_text":
      return (
        <Body>
          <Label htmlFor={`${id}-v`}>{label}</Label>
          <SelectBox id={`${id}-v`} className={field} value={String(rule.value)} onChange={(e) => onChange({ ...rule, value: e.target.value === "true" })} disabled={disabled}>
            <option value="true">{rule.field === "has_text" ? "Has text" : "Has a location"}</option>
            <option value="false">{rule.field === "has_text" ? "Has no text" : "Has no location"}</option>
          </SelectBox>
        </Body>
      );
    case "caption":
      return (
        <Body>
          <Label htmlFor={`${id}-v`}>{label}</Label>
          <input id={`${id}-v`} className={cn(inputClass, field)} value={rule.text} maxLength={200} onChange={(e) => onChange({ ...rule, text: e.target.value })} disabled={disabled} />
        </Body>
      );
    case "people":
      return (
        <Body>
          {people !== null && people.length === 0 && rule.ids.length === 0 ? (
            <>
              <span className="text-[0.8125rem] font-medium text-muted">{label}</span>
              <span className="text-sm text-muted">{NO_NAMED_PEOPLE}</span>
            </>
          ) : (
            <PeoplePicker
              label={`People together (rule ${n})`}
              people={people ?? []}
              value={rule.ids}
              max={MAX_FILTER_PEOPLE}
              onChange={(ids) => onChange({ ...rule, ids })}
              disabled={disabled}
            />
          )}
        </Body>
      );
  }
}

function Label({ htmlFor, children }: { htmlFor?: string; children: string }) {
  return (
    <label htmlFor={htmlFor} className="text-[0.8125rem] font-medium text-muted">
      {children}
    </label>
  );
}

function Body({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("flex flex-col gap-1.5", className)}>{children}</div>;
}
