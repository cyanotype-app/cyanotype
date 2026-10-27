//! The rules of a smart album (`docs/library.md`, "Smart albums"): validated, stored as JSON in `albums.rules`, and turned into the
//! very filters the search uses (`search::Filters`), so a rule and the same filter in a search are one piece of SQL.
//!
//! ```json
//! {"match": "all" | "any", "archived": "include" | "exclude",
//!  "rules": [{"field": "date", "from": "2024-01-01", "to": "2024-12-31"},
//!            {"field": "place" | "camera" | "lens", "value": "Lisbon"},
//!            {"field": "type", "value": "jpeg" | "heic" | "raw" | "other"},
//!            {"field": "favorite", "value": true},
//!            {"field": "rating", "min": 4},
//!            {"field": "tags", "names": ["beach", "sunset"], "mode": "all" | "any"},
//!            {"field": "people", "ids": ["<person id>"]},
//!            {"field": "pets", "kind": "any" | "dog" | "cat"},
//!            {"field": "has_location" | "has_text", "value": true | false},
//!            {"field": "caption", "text": "birthday"}]}
//! ```
//!
//! `match` joins the rules (all of them, or any); `archived` is not a rule but a switch for the whole album (archived photos are in
//! by default, as in a search), so "exclude" never turns into "or not archived". A text query (a semantic search such as "dog on the
//! beach") is deliberately **not** a rule: its ranking depends on the model and on the library, so the album's contents would
//! change without a photo changing. The caption rule is a plain substring, which is stable.
//!
//! The **people** rule is "all of these people are in the photo" (their ids, the owner's own: the album is checked when it is saved, an
//! id that is not the owner's is a `404`). It needs People (`FACES_ENABLED`): saving one while it is off is a `404`, and a saved rule
//! matches no photo while it is off. The album keeps only ids, never a name, so renaming a person changes nothing; a deleted person
//! leaves the rule matching nothing, and a merged one is not followed (edit the rule).
//!
//! The **pets** rule is "a dog, a cat or either was found in the photo" (`kind`: `any`, `dog` or `cat`). It needs Pets (`PETS_ENABLED`):
//! saving one while it is off is a `404`, and a saved rule matches no photo while it is off. The album keeps the kind, nothing else.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::Date;
use uuid::Uuid;

use super::search::{FileType, Filters, MAX_PEOPLE_FILTERS};
use crate::Features;
use cyanotype_core::pets::Which;
use cyanotype_service::{ApiError, ApiResult};

/// Most rules in one album.
pub(super) const MAX_RULES: usize = 20;
const MAX_TEXT: usize = 200;
/// Most tag names in a tags rule (the search filter's limit).
const MAX_TAGS: usize = 20;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Match {
    #[default]
    All,
    Any,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Archived {
    #[default]
    Include,
    Exclude,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum TagMode {
    #[default]
    All,
    Any,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "field", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Rule {
    Date {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    Place {
        value: String,
    },
    Camera {
        value: String,
    },
    Lens {
        value: String,
    },
    Type {
        value: String,
    },
    Favorite {
        value: bool,
    },
    Rating {
        min: i16,
    },
    Tags {
        names: Vec<String>,
        #[serde(default)]
        mode: TagMode,
    },
    HasLocation {
        value: bool,
    },
    HasText {
        value: bool,
    },
    Caption {
        text: String,
    },
    /// Every one of these people is in the photo.
    People {
        ids: Vec<Uuid>,
    },
    /// A dog, a cat or either was found in the photo: `any`, `dog` or `cat`.
    Pets {
        kind: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Rules {
    #[serde(default, rename = "match")]
    pub(super) join: Match,
    #[serde(default)]
    pub(super) archived: Archived,
    pub(super) rules: Vec<Rule>,
}

/// Words a rule's `field` must not be: a semantic text query.
const TEXT_QUERY_FIELDS: &[&str] = &["q", "query", "text", "search", "semantic", "text_query", "words"];

const NO_TEXT_QUERY: &str = "a text search (like \"dog on the beach\") cannot be a rule: its ranking is not stable, so the album would change without a photo changing. Use the caption or tag rules, or a filter";

impl Rules {
    /// Read and check what a client sent: a text query gets its own explanation, anything else wrong is a plain 400.
    pub(super) fn from_json(v: &Value) -> ApiResult<Rules> {
        if let Some(list) = v.get("rules").and_then(Value::as_array) {
            for r in list {
                let field = r.get("field").and_then(Value::as_str).unwrap_or("");
                if TEXT_QUERY_FIELDS.contains(&field.to_ascii_lowercase().as_str()) {
                    return Err(ApiError::BadRequest(NO_TEXT_QUERY.into()));
                }
            }
        }
        if v.get("q").is_some() || v.get("query").is_some() {
            return Err(ApiError::BadRequest(NO_TEXT_QUERY.into()));
        }
        let rules: Rules = serde_json::from_value(v.clone())
            .map_err(|e| ApiError::BadRequest(format!("the album's rules are not valid: {e}")))?;
        rules.validated()
    }

    /// Normalised copy: strings trimmed, tag names folded, limits applied. Errors say which rule.
    pub(super) fn validated(mut self) -> ApiResult<Rules> {
        let bad = |m: String| ApiError::BadRequest(m);
        if self.rules.is_empty() {
            return Err(bad("add at least one rule".into()));
        }
        if self.rules.len() > MAX_RULES {
            return Err(bad(format!("at most {MAX_RULES} rules")));
        }
        let text = |what: &str, s: &str| -> ApiResult<String> {
            let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if t.is_empty() {
                return Err(ApiError::BadRequest(format!("the {what} rule needs a value")));
            }
            if t.chars().count() > MAX_TEXT {
                return Err(ApiError::BadRequest(format!(
                    "the {what} rule is at most {MAX_TEXT} characters"
                )));
            }
            if t.chars().any(char::is_control) {
                return Err(ApiError::BadRequest(format!(
                    "the {what} rule cannot contain control characters"
                )));
            }
            Ok(t)
        };
        for r in &mut self.rules {
            match r {
                Rule::Date { from, to } => {
                    let day = |v: &Option<String>, n: &str| -> ApiResult<Option<Date>> {
                        v.as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|s| {
                                Date::parse(s, &time::macros::format_description!("[year]-[month]-[day]")).map_err(
                                    |_| {
                                        ApiError::BadRequest(format!(
                                            "the date rule's {n} must be a day like 2024-07-31"
                                        ))
                                    },
                                )
                            })
                            .transpose()
                    };
                    let (f, t) = (day(from, "from")?, day(to, "to")?);
                    if f.is_none() && t.is_none() {
                        return Err(bad("the date rule needs a from or a to day".into()));
                    }
                    if let (Some(f), Some(t)) = (f, t)
                        && f > t
                    {
                        return Err(bad("the date rule's from is after its to".into()));
                    }
                    *from = f.map(|d| d.to_string());
                    *to = t.map(|d| d.to_string());
                }
                Rule::Place { value } => *value = text("place", value)?,
                Rule::Camera { value } => *value = text("camera", value)?,
                Rule::Lens { value } => *value = text("lens", value)?,
                Rule::Type { value } => {
                    let v = value.trim().to_ascii_lowercase();
                    *value = match v.as_str() {
                        "jpeg" | "jpg" => "jpeg",
                        "heic" | "heif" | "avif" => "heic",
                        "raw" => "raw",
                        "other" => "other",
                        _ => return Err(bad("the type rule must be jpeg, heic, raw or other".into())),
                    }
                    .into();
                }
                Rule::Favorite { value } => {
                    if !*value {
                        return Err(bad("the favorite rule can only ask for favorites".into()));
                    }
                }
                Rule::Rating { min } => {
                    if !(1..=5).contains(min) {
                        return Err(bad("the rating rule needs 1 to 5 stars".into()));
                    }
                }
                Rule::Tags { names, .. } => {
                    let mut out: Vec<String> = Vec::new();
                    for n in names.iter() {
                        let n = cyanotype_core::tags::normalize(n)
                            .map_err(|e| ApiError::BadRequest(format!("the tags rule: {e}")))?;
                        if !out.iter().any(|o| o.to_lowercase() == n.to_lowercase()) {
                            out.push(n);
                        }
                    }
                    if out.is_empty() || out.len() > MAX_TAGS {
                        return Err(bad(format!("the tags rule needs 1 to {MAX_TAGS} tag names")));
                    }
                    *names = out;
                }
                Rule::HasLocation { .. } | Rule::HasText { .. } => {}
                Rule::Caption { text: t } => *t = text("caption", t)?,
                Rule::People { ids } => {
                    let mut out: Vec<Uuid> = Vec::new();
                    for id in ids.iter() {
                        if !out.contains(id) {
                            out.push(*id);
                        }
                    }
                    if out.is_empty() || out.len() > MAX_PEOPLE_FILTERS {
                        return Err(bad(format!("the people rule needs 1 to {MAX_PEOPLE_FILTERS} people")));
                    }
                    *ids = out;
                }
                Rule::Pets { kind } => {
                    let which =
                        Which::parse(kind).ok_or_else(|| bad("the pets rule must be any, dog or cat".into()))?;
                    *kind = which.name().to_string();
                }
            }
        }
        Ok(self)
    }

    /// The people the rules name (to check that they are the owner's, and that People is on).
    pub(super) fn people(&self) -> Vec<Uuid> {
        self.rules
            .iter()
            .filter_map(|r| match r {
                Rule::People { ids } => Some(ids.iter().copied()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Does a rule ask for pets (to check that Pets is on)?
    pub(super) fn has_pets(&self) -> bool {
        self.rules.iter().any(|r| matches!(r, Rule::Pets { .. }))
    }

    /// The filters of the album: one `Filters` per rule, joined as `match` says, with the archived switch. While People is off
    /// a people rule matches nothing, and likewise a pets rule while Pets is off.
    pub(super) fn filters(&self, features: Features) -> Filters {
        let members = self
            .rules
            .iter()
            .map(|r| {
                let f = Filters::rule();
                match r {
                    Rule::Date { from, to } => {
                        let parse = |s: &Option<String>| {
                            s.as_deref().and_then(|s| {
                                Date::parse(s, &time::macros::format_description!("[year]-[month]-[day]")).ok()
                            })
                        };
                        f.with_dates(parse(from), parse(to))
                    }
                    Rule::Place { value } => f.with_place(value.clone()),
                    Rule::Camera { value } => f.with_camera(value.clone()),
                    Rule::Lens { value } => f.with_lens(value.clone()),
                    Rule::Type { value } => f.with_kind(match value.as_str() {
                        "jpeg" => FileType::Jpeg,
                        "heic" => FileType::Heic,
                        "raw" => FileType::Raw,
                        _ => FileType::Other,
                    }),
                    Rule::Favorite { .. } => f.with_favorite(),
                    Rule::Rating { min } => f.with_rating(*min),
                    Rule::Tags { names, mode } => {
                        f.with_tags(names.iter().map(|n| n.to_lowercase()).collect(), *mode == TagMode::Any)
                    }
                    Rule::HasLocation { value } => f.with_has_location(*value),
                    Rule::HasText { value } => f.with_has_text(*value),
                    Rule::Caption { text } => f.with_caption(text.clone()),
                    Rule::People { .. } if !features.faces => f.nothing(),
                    Rule::People { ids } => f.with_people(ids.clone()),
                    Rule::Pets { .. } if !features.pets => f.nothing(),
                    Rule::Pets { kind } => match Which::parse(kind) {
                        Some(w) => f.with_pets(w),
                        None => f.nothing(),
                    },
                }
            })
            .collect();
        Filters::smart(self.join == Match::All, self.archived == Archived::Exclude, members)
    }

    pub(super) fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("rules serialise")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: Value) -> ApiResult<Rules> {
        Rules::from_json(&v)
    }

    #[test]
    fn a_valid_album_is_normalised_and_round_trips() {
        let r = parse(json!({"match": "any", "archived": "exclude", "rules": [
            {"field": "date", "from": " 2024-01-01 "},
            {"field": "place", "value": "  Lisbon  "},
            {"field": "type", "value": "JPG"},
            {"field": "tags", "names": ["Beach", "beach", "  sun   set "], "mode": "any"},
            {"field": "rating", "min": 4},
            {"field": "favorite", "value": true},
            {"field": "has_location", "value": false},
            {"field": "has_text", "value": true},
            {"field": "caption", "text": "birthday  cake"}
        ]}))
        .unwrap();
        let again = parse(r.to_json()).unwrap();
        assert_eq!(r, again);
        let j = r.to_json();
        assert_eq!(j["rules"][0], json!({"field": "date", "from": "2024-01-01"}));
        assert_eq!(j["rules"][1]["value"], "Lisbon");
        assert_eq!(j["rules"][2]["value"], "jpeg");
        assert_eq!(j["rules"][3]["names"], json!(["Beach", "sun set"]));
        assert_eq!(
            (j["match"].as_str(), j["archived"].as_str()),
            (Some("any"), Some("exclude"))
        );
        // Defaults: all, archived included.
        let d = parse(json!({"rules": [{"field": "favorite", "value": true}]})).unwrap();
        assert_eq!((d.join, d.archived), (Match::All, Archived::Include));
    }

    #[test]
    fn a_text_query_is_refused_with_its_own_explanation() {
        for v in [
            json!({"rules": [{"field": "query", "value": "dog on the beach"}]}),
            json!({"rules": [{"field": "text", "text": "dog"}]}),
            json!({"rules": [{"field": "semantic", "value": "dog"}]}),
            json!({"q": "dog", "rules": [{"field": "favorite", "value": true}]}),
        ] {
            let Err(ApiError::BadRequest(m)) = parse(v.clone()) else {
                panic!("{v} should be refused")
            };
            assert!(m.contains("text search") && m.contains("ranking is not stable"), "{m}");
        }
    }

    #[test]
    fn nonsense_is_a_plain_error() {
        for v in [
            json!({"rules": []}),
            json!({}),
            json!({"rules": [{"field": "nope", "value": 1}]}),
            json!({"rules": [{"field": "place", "value": "  "}]}),
            json!({"rules": [{"field": "place", "value": "x".repeat(201)}]}),
            json!({"rules": [{"field": "place", "value": "a", "extra": 1}]}),
            json!({"rules": [{"field": "date"}]}),
            json!({"rules": [{"field": "date", "from": "yesterday"}]}),
            json!({"rules": [{"field": "date", "from": "2024-05-02", "to": "2024-05-01"}]}),
            json!({"rules": [{"field": "type", "value": "gif"}]}),
            json!({"rules": [{"field": "rating", "min": 0}]}),
            json!({"rules": [{"field": "rating", "min": 6}]}),
            json!({"rules": [{"field": "favorite", "value": false}]}),
            json!({"rules": [{"field": "tags", "names": []}]}),
            json!({"rules": [{"field": "tags", "names": ["a,b"]}]}),
            json!({"match": "some", "rules": [{"field": "favorite", "value": true}]}),
            json!({"archived": "only", "rules": [{"field": "favorite", "value": true}]}),
            json!({"rules": (0..21).map(|_| json!({"field": "favorite", "value": true})).collect::<Vec<_>>()}),
        ] {
            assert!(matches!(parse(v.clone()), Err(ApiError::BadRequest(_))), "{v}");
        }
    }
}
