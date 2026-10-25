// Pure helpers of smart albums: the shape of a rule, a blank one per kind, what a rule or a whole album says in words, and the
// check the editor runs before it asks the server. The server's own validation (`smart.rs`) is the last word. Unit tested.
import type { SmartRule, SmartRules } from "./api";
import { MAX_FILTER_PEOPLE, peopleRuleLabel } from "./peopleText";
import { ratingFilterLabel } from "./ratingText";
import { nameProblem } from "./tagText";

export const MAX_RULES = 20;

export type RuleField = SmartRule["field"];

/** The kinds of rule, in the order the editor offers them, with their labels. */
export const RULE_FIELDS: { field: RuleField; label: string }[] = [
  { field: "date", label: "Date range" },
  { field: "place", label: "Place" },
  { field: "camera", label: "Camera" },
  { field: "lens", label: "Lens" },
  { field: "type", label: "File type" },
  { field: "favorite", label: "Favorites" },
  { field: "rating", label: "Rating" },
  { field: "tags", label: "Tags" },
  { field: "has_location", label: "Location" },
  { field: "has_text", label: "Text in the photo" },
  { field: "caption", label: "Caption contains" },
  { field: "people", label: "People" },
];

/** The kinds of rule to offer: People's only while People is on. */
export const ruleFieldsFor = (people: boolean) => RULE_FIELDS.filter((f) => f.field !== "people" || people);

/** Why a semantic text search cannot be a rule; the editor shows it next to "Add rule". */
export const NO_TEXT_QUERY_NOTE =
  "A text search like “dog on the beach” cannot be a rule: its ranking is not stable, so the album would change without a photo changing. Use a caption, tag or other rule instead.";

export function blankRule(field: RuleField): SmartRule {
  switch (field) {
    case "date":
      return { field, from: "", to: "" };
    case "place":
    case "camera":
    case "lens":
      return { field, value: "" };
    case "type":
      return { field, value: "jpeg" };
    case "favorite":
      return { field, value: true };
    case "rating":
      return { field, min: 4 };
    case "tags":
      return { field, names: [], mode: "all" };
    case "has_location":
    case "has_text":
      return { field, value: true };
    case "caption":
      return { field, text: "" };
    case "people":
      return { field, ids: [] };
  }
}

export const blankRules = (): SmartRules => ({ match: "all", archived: "include", rules: [blankRule("favorite")] });

const TYPE_WORD = { jpeg: "JPEG", heic: "HEIC", raw: "RAW", other: "other files" } as const;

/** One rule in words: "Place: Lisbon", "4 stars & up", "Tags: beach and sunset". */
export function ruleLabel(r: SmartRule): string {
  switch (r.field) {
    case "date": {
      if (r.from && r.to) return `${r.from} to ${r.to}`;
      return r.from ? `From ${r.from}` : `Until ${r.to ?? ""}`;
    }
    case "place":
      return `Place: ${r.value}`;
    case "camera":
      return `Camera: ${r.value}`;
    case "lens":
      return `Lens: ${r.value}`;
    case "type":
      return `Type: ${TYPE_WORD[r.value as keyof typeof TYPE_WORD] ?? r.value}`;
    case "favorite":
      return "Favorites";
    case "rating":
      return ratingFilterLabel(r.min);
    case "tags":
      return `Tags: ${r.names.join(r.mode === "any" ? " or " : " and ")}`;
    case "has_location":
      return r.value ? "Has a location" : "No location";
    case "has_text":
      return r.value ? "Has text in the photo" : "No text in the photo";
    case "caption":
      return `Caption has “${r.text}”`;
    case "people":
      return peopleRuleLabel(r.ids);
  }
}

/** The whole album in a line: "Favorites and 4 stars & up" / "Place: Lisbon or Place: Porto (without archived)". */
export function describeRules(rules: SmartRules): string {
  const joiner = rules.match === "any" ? " or " : " and ";
  const body = rules.rules.map(ruleLabel).join(joiner);
  return rules.archived === "exclude" ? `${body} (without archived)` : body;
}

/** What is wrong with the rules before they are sent, or null. */
export function rulesProblem(rules: SmartRules): string | null {
  if (rules.rules.length === 0) return "Add at least one rule.";
  if (rules.rules.length > MAX_RULES) return `At most ${MAX_RULES} rules.`;
  for (const r of rules.rules) {
    switch (r.field) {
      case "date":
        if (!r.from && !r.to) return "A date rule needs a start or an end day.";
        if (r.from && r.to && r.from > r.to) return "A date rule starts after it ends.";
        break;
      case "place":
      case "camera":
      case "lens":
        if (!r.value.trim()) return `The ${r.field} rule needs a value.`;
        break;
      case "caption":
        if (!r.text.trim()) return "The caption rule needs some text.";
        break;
      case "people":
        if (r.ids.length === 0) return "The people rule needs at least one person.";
        if (r.ids.length > MAX_FILTER_PEOPLE) return `The people rule takes at most ${MAX_FILTER_PEOPLE} people.`;
        break;
      case "tags": {
        if (r.names.length === 0) return "The tags rule needs at least one tag.";
        const bad = r.names.map(nameProblem).find(Boolean);
        if (bad) return bad;
        break;
      }
      default:
    }
  }
  return null;
}

/** The rules as the server wants them: empty dates and blank text left out. */
export function cleanRules(rules: SmartRules): SmartRules {
  return {
    ...rules,
    rules: rules.rules.map((r) => {
      if (r.field === "date") {
        const { field, from, to } = r;
        return { field, ...(from ? { from } : {}), ...(to ? { to } : {}) };
      }
      return r;
    }),
  };
}

/** "1,234 photos", "10,000+ photos" for a capped smart count. */
export function smartCount(n: number, capped: boolean | undefined): string {
  const t = `${n.toLocaleString("en-US")}${capped ? "+" : ""}`;
  return `${t} ${n === 1 && !capped ? "photo" : "photos"}`;
}
