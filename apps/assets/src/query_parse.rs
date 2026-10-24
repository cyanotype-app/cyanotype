//! Understanding dates and places in a search query ("beach june 2023", "dog in Lisbon", "snow last year").
//!
//! The words that name a date or one of the user's own places become filters; the rest is what the model searches for.
//! `search.rs` echoes what was understood as removable chips, and `skip=date` / `skip=place` turns one kind off again for
//! that query, so a wrong guess costs one click.
//!
//! **Dates** (UTC, like every date filter):
//! - a year: `2023` (1900 up to this year);
//! - a month with its year: `June 2023`, `jun 2023`, `June of 2023`, `2023-06`; a day: `5 June 2023`, `June 5th 2023`,
//!   `2023-06-05`; a range of years `2019-2021`;
//! - a season with its year: `summer 2022` = 1 Jun to 31 Aug (northern hemisphere: spring Mar-May, summer Jun-Aug, autumn or
//!   fall Sep-Nov, winter Dec-Feb, and `winter 2022` starts in December 2022);
//! - relative: `today`, `yesterday`, `this week`, `last week` (Monday to Sunday), `this month`, `last month`, `this year`,
//!   `last year`, and `last summer` and the like (the most recent finished season; `this summer`: the one that is on).
//!
//! A month or a season **needs its year**: "june" and "summer" alone are far more often part of what is photographed
//! ("summer dress", "june bug") than a date, and a month of any year is not a range. A preposition right before a date
//! ("in", "from", "during", "of", "since", "on") is taken with it.
//!
//! **Places** are only those in the user's own library (the geocoded city, region and country names of their photos), so a
//! word the library has never seen cannot become a filter. A place counts when it is introduced by "in", "at", "near",
//! "around", "from", "across" or "within" ("dog in Lisbon"), or is the last words of the query ("sunset Maui"). Words that are
//! also ordinary words or common names ([`AMBIGUOUS`]: nice, reading, victoria, may, march...) count only after one of those
//! prepositions: "in Nice" is the city, "nice view" is a view, "kid reading" is not Reading. Country codes are never matched.
//!
//! **People** are only those the user has named and not hidden (`FACES_ENABLED`; the caller passes none while it is off). A name
//! counts when it is introduced by "with", "of", "and", "featuring", "plus" or "including" ("beach with Person A"), or is the first or
//! the last words of the query ("Person A beach", "beach Person A"): "Person A and Person B 2023" is both, together. The longest name
//! wins ("Ann Lee" over "Ann"). A name two people share is never matched (it would be a guess), a name of fewer than three letters is
//! not matched, and a single word that is also an ordinary word, a month or a place often enough ([`COMMON_NAMES`], [`AMBIGUOUS`], the
//! filler words) counts only after one of those introducing words: "with Rose" is a person, "rose garden" is a garden. A date or a
//! place that was read first keeps its words.
//!
//! If only filler words are left once the filters are taken out ("photos from 2023"), the search is a plain filter: the
//! timeline of that year.
use time::{Date, Duration, Month, OffsetDateTime};
use uuid::Uuid;

/// A place in the user's library, as the place facet lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub value: String,
    pub kind: PlaceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceKind {
    City,
    Region,
    Country,
}

/// A person the user has named (and not hidden), as the search parser knows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub id: Uuid,
    pub name: String,
}

/// Which kinds of understanding are off for this query (`skip=date,place,people`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Skip {
    pub date: bool,
    pub place: bool,
    pub people: bool,
}

impl Skip {
    pub fn parse(s: Option<&str>) -> Skip {
        let mut skip = Skip::default();
        for part in s.unwrap_or("").split(',').map(|p| p.trim().to_ascii_lowercase()) {
            match part.as_str() {
                "date" => skip.date = true,
                "place" => skip.place = true,
                "people" => skip.people = true,
                _ => {}
            }
        }
        skip
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateFilter {
    /// What the chip says: "June 2023", "Summer 2022", "Last year".
    pub label: String,
    /// The words of the query it came from ("in june 2023").
    pub matched: String,
    /// First and last day, inclusive.
    pub from: Date,
    pub to: Date,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceFilter {
    /// The place as the library spells it ("Maui"); this is the `place` filter's value.
    pub value: String,
    pub kind: PlaceKind,
    pub matched: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonFilter {
    pub id: Uuid,
    /// The name as the owner wrote it ("Person A").
    pub name: String,
    pub matched: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// What is left to search for (empty: a filter-only search).
    pub text: String,
    pub date: Option<DateFilter>,
    pub place: Option<PlaceFilter>,
    /// The named people in the query, in the order they were written: photos where they all are.
    pub people: Vec<PersonFilter>,
}

/// Most people one search can name (the `people` filter's limit).
pub const MAX_PEOPLE: usize = 10;

/// Single words that are also ordinary words, months or common given names, and are cities or regions often enough to be in
/// a library: they are a place only after a preposition ("in nice").
pub const AMBIGUOUS: &[&str] = &[
    "nice",
    "reading",
    "bath",
    "may",
    "march",
    "august",
    "june",
    "april",
    "jersey",
    "turkey",
    "mobile",
    "orange",
    "hull",
    "derby",
    "wells",
    "bury",
    "cork",
    "rock",
    "victoria",
    "florence",
    "georgia",
    "jordan",
    "chad",
    "charlotte",
    "austin",
    "jackson",
    "lincoln",
    "madison",
    "chelsea",
    "china",
    "aurora",
    "paradise",
    "buffalo",
    "phoenix",
    "providence",
    "eden",
    "hope",
    "bend",
    "normal",
    "mesa",
    "salem",
    "lancaster",
    "granada",
    "split",
    "stoke",
];

/// Words that introduce a name ("with Person A", "Person A and Person B").
const PEOPLE_INTRO: &[&str] = &["with", "of", "and", "featuring", "plus", "including"];
/// Single words that are also people's names and ordinary words (or what a family calls its members and pets): a name like these is
/// a person only after one of [`PEOPLE_INTRO`], "with Rose", not "rose garden".
pub const COMMON_NAMES: &[&str] = &[
    "will", "mark", "rose", "grace", "joy", "faith", "summer", "autumn", "winter", "spring", "dawn", "bill", "pat",
    "art", "rich", "frank", "sue", "dean", "gene", "bob", "jack", "pearl", "ruby", "amber", "violet", "lily", "daisy",
    "holly", "ivy", "olive", "sandy", "penny", "crystal", "sky", "star", "baby", "mom", "mum", "dad", "mama", "papa",
    "nana", "grandma", "grandpa", "uncle", "aunt", "friend", "friends", "family", "kid", "kids", "dog", "cat", "puppy",
    "kitten", "bird", "horse", "beach", "sunset",
];
const PLACE_PREPOSITIONS: &[&str] = &["in", "at", "near", "around", "from", "across", "within"];
const DATE_PREPOSITIONS: &[&str] = &["in", "from", "during", "of", "since", "on", "at", "around"];
/// Words that mean nothing on their own once a filter has been taken out of the query.
const FILLER: &[&str] = &[
    "photo", "photos", "picture", "pictures", "pic", "pics", "image", "images", "shot", "shots", "of", "from", "in",
    "on", "at", "the", "a", "an", "my", "all", "me", "show", "with", "and", "taken", "took", "during", "near",
    "around", "pictures",
];

struct Tok {
    raw: String,
    /// Lower case, punctuation trimmed from both ends (inner `-` and `'` stay: `2023-06`).
    core: String,
    /// Lower case with everything that is not a letter or digit as a space (for comparing with place names).
    fold: String,
    used: bool,
}

fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn tokenize(q: &str) -> Vec<Tok> {
    q.split_whitespace()
        .map(|raw| {
            let core = raw.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            Tok {
                fold: fold(raw),
                raw: raw.to_string(),
                core,
                used: false,
            }
        })
        .collect()
}

fn month_of(s: &str) -> Option<Month> {
    Some(match s {
        "january" | "jan" => Month::January,
        "february" | "feb" => Month::February,
        "march" | "mar" => Month::March,
        "april" | "apr" => Month::April,
        "may" => Month::May,
        "june" | "jun" => Month::June,
        "july" | "jul" => Month::July,
        "august" | "aug" => Month::August,
        "september" | "sep" | "sept" => Month::September,
        "october" | "oct" => Month::October,
        "november" | "nov" => Month::November,
        "december" | "dec" => Month::December,
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

fn season_of(s: &str) -> Option<Season> {
    Some(match s {
        "spring" => Season::Spring,
        "summer" => Season::Summer,
        "autumn" | "fall" => Season::Autumn,
        "winter" => Season::Winter,
        _ => return None,
    })
}

impl Season {
    fn name(self) -> &'static str {
        match self {
            Season::Spring => "Spring",
            Season::Summer => "Summer",
            Season::Autumn => "Autumn",
            Season::Winter => "Winter",
        }
    }

    /// The season that starts in `year` (northern hemisphere).
    fn range(self, year: i32) -> Option<(Date, Date)> {
        let (m1, m2) = match self {
            Season::Spring => (Month::March, Month::May),
            Season::Summer => (Month::June, Month::August),
            Season::Autumn => (Month::September, Month::November),
            Season::Winter => (Month::December, Month::February),
        };
        let end_year = if self == Season::Winter { year + 1 } else { year };
        Some((Date::from_calendar_date(year, m1, 1).ok()?, last_day(end_year, m2)?))
    }
}

fn last_day(year: i32, month: Month) -> Option<Date> {
    let next = if month == Month::December {
        Date::from_calendar_date(year + 1, Month::January, 1).ok()?
    } else {
        Date::from_calendar_date(year, month.next(), 1).ok()?
    };
    next.previous_day()
}

fn month_name(m: Month) -> &'static str {
    match m {
        Month::January => "January",
        Month::February => "February",
        Month::March => "March",
        Month::April => "April",
        Month::May => "May",
        Month::June => "June",
        Month::July => "July",
        Month::August => "August",
        Month::September => "September",
        Month::October => "October",
        Month::November => "November",
        Month::December => "December",
    }
}

fn year_of(s: &str, today: Date) -> Option<i32> {
    (s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse::<i32>().ok())
        .flatten()
        .filter(|y| (1900..=today.year()).contains(y))
}

fn day_of(s: &str) -> Option<u8> {
    let digits = s
        .strip_suffix("st")
        .or_else(|| s.strip_suffix("nd"))
        .or_else(|| s.strip_suffix("rd"))
        .or_else(|| s.strip_suffix("th"))
        .unwrap_or(s);
    (digits.len() <= 2 && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse::<u8>().ok())
        .flatten()
        .filter(|d| (1..=31).contains(d))
}

fn month_range(year: i32, month: Month) -> Option<(Date, Date)> {
    Some((Date::from_calendar_date(year, month, 1).ok()?, last_day(year, month)?))
}

/// The date expression starting at `toks[i]`: its length in tokens, label, and range.
fn date_at(toks: &[Tok], i: usize, today: Date) -> Option<(usize, String, Date, Date)> {
    let c = |k: usize| toks.get(i + k).filter(|t| !t.used).map(|t| t.core.as_str());
    let w0 = c(0)?;
    // 2023-06-05, 2023-06, 2019-2021
    let parts: Vec<&str> = w0.split('-').collect();
    if parts
        .iter()
        .all(|p| p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty())
    {
        match parts.as_slice() {
            [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => {
                let date = Date::from_calendar_date(
                    year_of(y, today)?,
                    Month::try_from(m.parse::<u8>().ok()?).ok()?,
                    d.parse().ok()?,
                )
                .ok()?;
                return Some((
                    1,
                    format!("{} {} {}", date.day(), month_name(date.month()), date.year()),
                    date,
                    date,
                ));
            }
            [y, m] if y.len() == 4 && m.len() == 2 => {
                let (year, month) = (year_of(y, today)?, Month::try_from(m.parse::<u8>().ok()?).ok()?);
                let (a, b) = month_range(year, month)?;
                return Some((1, format!("{} {year}", month_name(month)), a, b));
            }
            [a, b] if a.len() == 4 && b.len() == 4 => {
                let (ya, yb) = (year_of(a, today)?, year_of(b, today)?);
                if ya <= yb {
                    return Some((
                        1,
                        format!("{ya}-{yb}"),
                        Date::from_calendar_date(ya, Month::January, 1).ok()?,
                        Date::from_calendar_date(yb, Month::December, 31).ok()?,
                    ));
                }
            }
            _ => {}
        }
    }
    // Month [day] [of] year
    if let Some(month) = month_of(w0) {
        let year_after = |k: usize| {
            let k = if c(k) == Some("of") { k + 1 } else { k };
            c(k).and_then(|w| year_of(w, today)).map(|y| (k, y))
        };
        // "june 5 2023": the day counts only with the year after it.
        if let Some(d) = c(1).and_then(day_of)
            && let Some((k, year)) = year_after(2)
        {
            let date = Date::from_calendar_date(year, month, d).ok()?;
            return Some((k + 1, format!("{} {} {year}", d, month_name(month)), date, date));
        }
        let (k, year) = year_after(1)?;
        let (a, b) = month_range(year, month)?;
        return Some((k + 1, format!("{} {year}", month_name(month)), a, b));
    }
    if let Some(d) = day_of(w0)
        && let Some(month) = c(1).and_then(month_of)
    {
        let mut k = 2;
        if c(k) == Some("of") {
            k += 1;
        }
        let year = year_of(c(k)?, today)?;
        let date = Date::from_calendar_date(year, month, d).ok()?;
        return Some((k + 1, format!("{} {} {year}", d, month_name(month)), date, date));
    }
    // Season [of] year
    if let Some(season) = season_of(w0) {
        let mut k = 1;
        if c(k) == Some("of") {
            k += 1;
        }
        let year = year_of(c(k)?, today)?;
        let (a, b) = season.range(year)?;
        return Some((k + 1, format!("{} {year}", season.name()), a, b));
    }
    // this / last + unit or season, yesterday, today
    match (w0, c(1)) {
        ("today", _) => return Some((1, "Today".into(), today, today)),
        ("yesterday", _) => {
            let d = today.previous_day()?;
            return Some((1, "Yesterday".into(), d, d));
        }
        ("last" | "this", Some(unit)) => {
            let last = w0 != "this";
            if unit == "year" {
                let y = today.year() - i32::from(last);
                return Some((
                    2,
                    if last { "Last year" } else { "This year" }.into(),
                    Date::from_calendar_date(y, Month::January, 1).ok()?,
                    Date::from_calendar_date(y, Month::December, 31).ok()?,
                ));
            }
            if unit == "month" {
                let (mut y, mut m) = (today.year(), today.month());
                if last {
                    if m == Month::January {
                        (y, m) = (y - 1, Month::December);
                    } else {
                        m = m.previous();
                    }
                }
                let (a, b) = month_range(y, m)?;
                return Some((2, if last { "Last month" } else { "This month" }.into(), a, b));
            }
            if unit == "week" {
                let monday = today - Duration::days(i64::from(today.weekday().number_days_from_monday()));
                let start = if last { monday - Duration::days(7) } else { monday };
                return Some((
                    2,
                    if last { "Last week" } else { "This week" }.into(),
                    start,
                    start + Duration::days(6),
                ));
            }
            if let Some(season) = season_of(unit) {
                // Seasons starting last year, this year and the year before; "last" is the latest one that has finished,
                // "this" the one that is on.
                let mut ranges: Vec<(Date, Date)> = (today.year() - 2..=today.year())
                    .filter_map(|y| season.range(y))
                    .collect();
                ranges.sort();
                let pick = if last {
                    ranges.iter().rev().find(|(_, b)| *b < today)
                } else {
                    ranges.iter().rev().find(|(a, b)| *a <= today && today <= *b)
                }?;
                let label = format!(
                    "{} {}",
                    if last { "Last" } else { "This" },
                    season.name().to_lowercase()
                );
                return Some((2, label, pick.0, pick.1));
            }
        }
        _ => {}
    }
    // A bare year.
    if let Some(y) = year_of(w0, today) {
        return Some((
            1,
            y.to_string(),
            Date::from_calendar_date(y, Month::January, 1).ok()?,
            Date::from_calendar_date(y, Month::December, 31).ok()?,
        ));
    }
    None
}

fn matched_text(toks: &[Tok], from: usize, to: usize) -> String {
    toks[from..to]
        .iter()
        .map(|t| t.raw.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_date(toks: &mut [Tok], today: Date) -> Option<DateFilter> {
    for i in 0..toks.len() {
        let Some((len, label, from, to)) = date_at(toks, i, today) else {
            continue;
        };
        let mut start = i;
        // "in june 2023", "from 2023": the preposition goes with the date.
        if i > 0 && !toks[i - 1].used && DATE_PREPOSITIONS.contains(&toks[i - 1].core.as_str()) {
            start = i - 1;
        }
        let matched = matched_text(toks, start, i + len);
        toks[start..i + len].iter_mut().for_each(|t| t.used = true);
        return Some(DateFilter {
            label,
            matched,
            from,
            to,
        });
    }
    None
}

fn find_place(toks: &mut [Tok], places: &[Place]) -> Option<PlaceFilter> {
    if places.is_empty() {
        return None;
    }
    let lookup: std::collections::HashMap<String, &Place> = places
        .iter()
        .filter(|p| fold(&p.value).chars().count() >= 3)
        .map(|p| (fold(&p.value), p))
        .collect();
    // Index of the last token not already taken (the end of the query, ignoring a date that was cut out of it).
    let last = toks.iter().rposition(|t| !t.used)?;
    let mut best: Option<(usize, usize, bool, &Place)> = None; // start, end (exclusive), introduced, place
    for start in 0..toks.len() {
        if toks[start].used {
            continue;
        }
        for len in (1..=4).rev() {
            let end = start + len;
            if end > toks.len() || toks[start..end].iter().any(|t| t.used || t.fold.is_empty()) {
                continue;
            }
            let name = toks[start..end]
                .iter()
                .map(|t| t.fold.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let Some(place) = lookup.get(&name) else { continue };
            let introduced =
                start > 0 && !toks[start - 1].used && PLACE_PREPOSITIONS.contains(&toks[start - 1].core.as_str());
            let trailing = end - 1 == last;
            let ambiguous = !name.contains(' ') && AMBIGUOUS.contains(&name.as_str());
            if introduced || (trailing && !ambiguous) {
                // An introduced place beats a trailing one; among equals the longest, then the leftmost, wins.
                let better = match best {
                    None => true,
                    Some((_, _, was_introduced, p)) => {
                        (introduced && !was_introduced)
                            || (introduced == was_introduced && fold(&place.value).len() > fold(&p.value).len())
                    }
                };
                if better {
                    best = Some((start, end, introduced, place));
                }
                break; // the longest window at this start
            }
        }
    }
    let (start, end, introduced, place) = best?;
    let from = if introduced { start - 1 } else { start };
    let matched = matched_text(toks, from, end);
    toks[from..end].iter_mut().for_each(|t| t.used = true);
    Some(PlaceFilter {
        value: place.value.clone(),
        kind: place.kind,
        matched,
    })
}

/// Is this (folded) name an ordinary word, a month, a season or a place often enough that it needs an introducing word?
fn is_common(name: &str) -> bool {
    !name.contains(' ')
        && (COMMON_NAMES.contains(&name)
            || AMBIGUOUS.contains(&name)
            || FILLER.contains(&name)
            || PLACE_PREPOSITIONS.contains(&name)
            || month_of(name).is_some()
            || season_of(name).is_some())
}

fn find_people(toks: &mut [Tok], people: &[Named]) -> Vec<PersonFilter> {
    if people.is_empty() {
        return Vec::new();
    }
    // A name two people share is never matched: None.
    let mut lookup: std::collections::HashMap<String, Option<&Named>> = std::collections::HashMap::new();
    for p in people {
        let f = fold(&p.name);
        if f.chars().count() >= 3 {
            lookup.entry(f).and_modify(|e| *e = None).or_insert(Some(p));
        }
    }
    // (position of the first word, person); put back in reading order at the end.
    let mut found: Vec<(usize, PersonFilter)> = Vec::new();
    // A second pass lets "beach Person A Person B" match: B is the last words, and then A is next to a name that was taken.
    loop {
        let before = found.len();
        let mut start = 0;
        while start < toks.len() && found.len() < MAX_PEOPLE {
            if toks[start].used {
                start += 1;
                continue;
            }
            let mut hit: Option<(usize, &Named, bool)> = None; // end (exclusive), person, introduced
            for len in (1..=4).rev() {
                let end = start + len;
                if end > toks.len() || toks[start..end].iter().any(|t| t.used || t.fold.is_empty()) {
                    continue;
                }
                let name = toks[start..end]
                    .iter()
                    .map(|t| t.fold.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let Some(entry) = lookup.get(&name) else { continue };
                // The longest name that is somebody's is the one meant: if that is a shared name, nothing is guessed here.
                let Some(person) = entry else { break };
                let introduced =
                    start > 0 && !toks[start - 1].used && PEOPLE_INTRO.contains(&toks[start - 1].core.as_str());
                let leading = toks[..start].iter().all(|t| t.used);
                let trailing = toks[end..].iter().all(|t| t.used);
                let ok = if is_common(&name) {
                    introduced
                } else {
                    introduced || leading || trailing
                };
                if ok {
                    hit = Some((end, person, introduced));
                }
                break;
            }
            let Some((end, person, introduced)) = hit else {
                start += 1;
                continue;
            };
            let from = if introduced { start - 1 } else { start };
            let matched = matched_text(toks, from, end);
            toks[from..end].iter_mut().for_each(|t| t.used = true);
            found.push((
                start,
                PersonFilter {
                    id: person.id,
                    name: person.name.clone(),
                    matched,
                },
            ));
            start = end;
        }
        if found.len() == before || found.len() >= MAX_PEOPLE {
            break;
        }
    }
    found.sort_by_key(|(at, _)| *at);
    // The same person named twice is one filter.
    let mut out: Vec<PersonFilter> = Vec::new();
    for (_, p) in found {
        if !out.iter().any(|o| o.id == p.id) {
            out.push(p);
        }
    }
    out
}

/// Take the dates and the user's own places out of `query` (the tests of those; the server reads people too: [`parse_with`]).
#[cfg(test)]
pub fn parse(query: &str, today: Date, places: &[Place], skip: Skip) -> Parsed {
    parse_with(query, today, places, &[], skip)
}

/// Take the dates, the user's own places and the people they have named out of `query` (a date, then a place, then the people).
pub fn parse_with(query: &str, today: Date, places: &[Place], people: &[Named], skip: Skip) -> Parsed {
    let mut toks = tokenize(query);
    let date = if skip.date { None } else { find_date(&mut toks, today) };
    let place = if skip.place {
        None
    } else {
        find_place(&mut toks, places)
    };
    let people = if skip.people {
        Vec::new()
    } else {
        find_people(&mut toks, people)
    };
    let rest: Vec<&Tok> = toks.iter().filter(|t| !t.used).collect();
    let filtered = date.is_some() || place.is_some() || !people.is_empty();
    let only_filler = rest
        .iter()
        .all(|t| t.core.is_empty() || FILLER.contains(&t.core.as_str()));
    let text = if filtered && only_filler {
        String::new()
    } else {
        rest.iter().map(|t| t.raw.as_str()).collect::<Vec<_>>().join(" ")
    };
    Parsed {
        text,
        date,
        place,
        people,
    }
}

/// Today in UTC.
pub fn today() -> Date {
    OffsetDateTime::now_utc().date()
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    /// A Wednesday.
    const TODAY: Date = date!(2026 - 10 - 14);

    fn places() -> Vec<Place> {
        let p = |v: &str, kind| Place { value: v.into(), kind };
        vec![
            p("Maui", PlaceKind::City),
            p("Hawaii", PlaceKind::Region),
            p("Lisbon", PlaceKind::City),
            p("Portugal", PlaceKind::Country),
            p("Nice", PlaceKind::City),
            p("Reading", PlaceKind::City),
            p("Victoria", PlaceKind::City),
            p("New York", PlaceKind::City),
            p("York", PlaceKind::City),
            p("Paris", PlaceKind::City),
            p("San Francisco", PlaceKind::City),
            p("St. John's", PlaceKind::City),
            p("Mayfair", PlaceKind::City),
            p("Of", PlaceKind::City),
        ]
    }

    fn run(q: &str) -> Parsed {
        parse(q, TODAY, &places(), Skip::default())
    }

    fn range(p: &Parsed) -> (Date, Date) {
        let d = p.date.as_ref().unwrap_or_else(|| panic!("no date in {p:?}"));
        (d.from, d.to)
    }

    #[test]
    fn years_months_and_days() {
        let p = run("beach 2023");
        assert_eq!(
            (p.text.as_str(), range(&p)),
            ("beach", (date!(2023 - 01 - 01), date!(2023 - 12 - 31)))
        );
        assert_eq!(p.date.as_ref().unwrap().label, "2023");
        let p = run("dog June 2023");
        assert_eq!(
            (p.text.as_str(), range(&p)),
            ("dog", (date!(2023 - 06 - 01), date!(2023 - 06 - 30)))
        );
        assert_eq!(p.date.as_ref().unwrap().label, "June 2023");
        assert_eq!(range(&run("dog jun 2023")), range(&p));
        assert_eq!(range(&run("dog in June of 2023")), range(&p));
        assert_eq!(run("dog in June of 2023").text, "dog");
        assert_eq!(
            range(&run("february 2024 snow")),
            (date!(2024 - 02 - 01), date!(2024 - 02 - 29)),
            "leap year"
        );
        assert_eq!(
            range(&run("december 2022")),
            (date!(2022 - 12 - 01), date!(2022 - 12 - 31))
        );
        assert_eq!(
            range(&run("5 june 2023")),
            (date!(2023 - 06 - 05), date!(2023 - 06 - 05))
        );
        assert_eq!(
            range(&run("cake june 5th, 2023")),
            (date!(2023 - 06 - 05), date!(2023 - 06 - 05))
        );
        assert_eq!(
            range(&run("2023-06-05")),
            (date!(2023 - 06 - 05), date!(2023 - 06 - 05))
        );
        assert_eq!(range(&run("2023-06")), (date!(2023 - 06 - 01), date!(2023 - 06 - 30)));
        assert_eq!(range(&run("2019-2021")), (date!(2019 - 01 - 01), date!(2021 - 12 - 31)));
        assert!(run("2021-2019 trip").date.is_none());
    }

    #[test]
    fn seasons_need_a_year_and_follow_the_northern_hemisphere() {
        assert_eq!(
            range(&run("hiking summer 2022")),
            (date!(2022 - 06 - 01), date!(2022 - 08 - 31))
        );
        assert_eq!(
            range(&run("spring 2021")),
            (date!(2021 - 03 - 01), date!(2021 - 05 - 31))
        );
        assert_eq!(
            range(&run("autumn 2021")),
            (date!(2021 - 09 - 01), date!(2021 - 11 - 30))
        );
        assert_eq!(
            range(&run("fall of 2021")),
            (date!(2021 - 09 - 01), date!(2021 - 11 - 30))
        );
        assert_eq!(
            range(&run("winter 2022")),
            (date!(2022 - 12 - 01), date!(2023 - 02 - 28))
        );
        assert_eq!(
            range(&run("winter 2023")),
            (date!(2023 - 12 - 01), date!(2024 - 02 - 29))
        );
        assert_eq!(run("summer 2022").date.unwrap().label, "Summer 2022");
        // Alone, a season is part of what is photographed.
        for q in [
            "summer dress",
            "winter landscape",
            "spring flowers",
            "autumn leaves",
            "fall",
        ] {
            let p = run(q);
            assert!(p.date.is_none(), "{q}");
            assert_eq!(p.text, q);
        }
    }

    #[test]
    fn relative_dates() {
        let r = |q: &str| range(&run(q));
        assert_eq!(r("last year"), (date!(2025 - 01 - 01), date!(2025 - 12 - 31)));
        assert_eq!(r("this year"), (date!(2026 - 01 - 01), date!(2026 - 12 - 31)));
        assert_eq!(r("last month"), (date!(2026 - 09 - 01), date!(2026 - 09 - 30)));
        assert_eq!(r("this month"), (date!(2026 - 10 - 01), date!(2026 - 10 - 31)));
        assert_eq!(r("last week"), (date!(2026 - 10 - 05), date!(2026 - 10 - 11)));
        assert_eq!(r("this week"), (date!(2026 - 10 - 12), date!(2026 - 10 - 18)));
        assert_eq!(r("yesterday"), (date!(2026 - 10 - 13), date!(2026 - 10 - 13)));
        assert_eq!(r("today"), (date!(2026 - 10 - 14), date!(2026 - 10 - 14)));
        assert_eq!(r("last summer"), (date!(2026 - 06 - 01), date!(2026 - 08 - 31)));
        assert_eq!(r("last winter"), (date!(2025 - 12 - 01), date!(2026 - 02 - 28)));
        assert_eq!(r("last spring"), (date!(2026 - 03 - 01), date!(2026 - 05 - 31)));
        assert_eq!(r("this autumn"), (date!(2026 - 09 - 01), date!(2026 - 11 - 30)));
        let p = run("sunset last year");
        assert_eq!(
            (p.text.as_str(), p.date.unwrap().label.as_str()),
            ("sunset", "Last year")
        );
        // January looks back into December of the year before.
        let jan = parse("last month", date!(2026 - 01 - 10), &[], Skip::default());
        assert_eq!(
            (jan.date.as_ref().unwrap().from, jan.date.unwrap().to),
            (date!(2025 - 12 - 01), date!(2025 - 12 - 31))
        );
        // "last" and "this" alone are words.
        assert!(run("last").date.is_none() && run("this is nice").date.is_none());
        assert!(run("last night").date.is_none());
    }

    #[test]
    fn ambiguous_words_are_not_dates_without_a_year() {
        for q in [
            "may",
            "march",
            "march of the penguins",
            "may i",
            "august light",
            "sept",
            "dec",
            "mar",
            "jun bug",
            "in may",
        ] {
            let p = run(q);
            assert!(p.date.is_none(), "{q}: {p:?}");
            assert_eq!(p.text, q);
        }
        assert_eq!(range(&run("May 2023")), (date!(2023 - 05 - 01), date!(2023 - 05 - 31)));
        assert_eq!(
            range(&run("march 2023")),
            (date!(2023 - 03 - 01), date!(2023 - 03 - 31))
        );
        assert_eq!(
            range(&run("in march 2023")),
            (date!(2023 - 03 - 01), date!(2023 - 03 - 31))
        );
        assert_eq!(run("in March 2023").text, "");
    }

    #[test]
    fn numbers_that_are_not_years_stay_in_the_text() {
        for q in [
            "1080p video",
            "2 dogs",
            "1899",
            "2027",
            "route 66",
            "10000 steps",
            "12345",
            "007",
        ] {
            let p = run(q);
            assert!(p.date.is_none(), "{q}: {p:?}");
            assert_eq!(p.text, q);
        }
        // This year is a year, next year is not.
        assert!(run("2026").date.is_some() && run("2027").date.is_none());
    }

    #[test]
    fn places_after_in_or_at_the_end() {
        let p = run("dog in Lisbon");
        assert_eq!(
            (p.text.as_str(), p.place.as_ref().map(|p| p.value.as_str())),
            ("dog", Some("Lisbon"))
        );
        assert_eq!(p.place.as_ref().unwrap().matched, "in Lisbon");
        let p = run("sunset maui");
        assert_eq!((p.text.as_str(), p.place.unwrap().value.as_str()), ("sunset", "Maui"));
        let p = run("MAUI");
        assert_eq!((p.text.as_str(), p.place.unwrap().value.as_str()), ("", "Maui"));
        let p = run("beach at Hawaii, sunset");
        assert_eq!(
            (p.text.as_str(), p.place.unwrap().value.as_str()),
            ("beach sunset", "Hawaii")
        );
        let p = run("food from portugal");
        assert_eq!((p.text.as_str(), p.place.unwrap().kind), ("food", PlaceKind::Country));
        // Not at the end and not introduced: part of the sentence.
        let p = run("maui sunset");
        assert!(p.place.is_none(), "{p:?}");
        // Places the library does not have are never filters.
        assert!(run("dog in Tokyo").place.is_none());
        assert_eq!(run("dog in Tokyo").text, "dog in Tokyo");
        assert!(parse("dog in Lisbon", TODAY, &[], Skip::default()).place.is_none());
    }

    #[test]
    fn multi_word_places_win_over_their_parts() {
        let p = run("pizza in New York");
        assert_eq!(
            (p.text.as_str(), p.place.unwrap().value.as_str()),
            ("pizza", "New York")
        );
        let p = run("pizza york");
        assert_eq!(p.place.unwrap().value, "York");
        let p = run("fog in San Francisco 2019");
        assert_eq!(
            (p.text.as_str(), p.place.unwrap().value.as_str()),
            ("fog", "San Francisco")
        );
        let p = run("church in St. John's");
        assert_eq!(
            (p.text.as_str(), p.place.unwrap().value.as_str()),
            ("church", "St. John's")
        );
    }

    #[test]
    fn ambiguous_place_names_need_in() {
        // The cases the feature must not get wrong: ordinary words that are also places in this library.
        for q in [
            "nice",
            "nice view",
            "a nice dog",
            "kid reading",
            "reading a book",
            "victoria",
            "queen victoria",
            "mayfair day",
            "dog nice",
        ] {
            let p = run(q);
            assert!(p.place.is_none(), "{q}: {p:?}");
            assert_eq!(p.text, q);
        }
        let p = run("dinner in Nice");
        assert_eq!((p.text.as_str(), p.place.unwrap().value.as_str()), ("dinner", "Nice"));
        let p = run("walk in Reading");
        assert_eq!(p.place.unwrap().value, "Reading");
        let p = run("harbour in victoria");
        assert_eq!(p.place.unwrap().value, "Victoria");
        // "may" is a month only with a year, and Mayfair is a place only when asked for as one.
        assert!(run("may 2023").place.is_none());
        // A place called "Of" is never found inside a sentence ("photo of": a preposition, not a place).
        assert!(run("photo of a dog").place.is_none());
        let p = run("photos of dogs");
        assert!(p.place.is_none() && p.date.is_none());
    }

    #[test]
    fn dates_and_places_together_and_filter_only_searches() {
        let p = run("dog in Lisbon June 2023");
        assert_eq!(p.text, "dog");
        assert_eq!(p.place.as_ref().unwrap().value, "Lisbon");
        assert_eq!(range(&p), (date!(2023 - 06 - 01), date!(2023 - 06 - 30)));
        let p = run("Maui 2023");
        assert_eq!(p.text, "");
        assert_eq!(
            p.place.as_ref().unwrap().value,
            "Maui",
            "the date is cut out first, so Maui is the end of what is left"
        );
        let p = run("photos from 2023");
        assert_eq!(p.text, "", "filler left over is a filter-only search");
        let p = run("pictures in Lisbon last year");
        assert_eq!((p.text.as_str(), p.place.is_some(), p.date.is_some()), ("", true, true));
        // Without a filter the text is never touched, filler or not.
        let p = run("photos of dogs");
        assert_eq!(p.text, "photos of dogs");
        // A date next to a word that merely looks like one.
        let p = run("2023 calendar");
        assert_eq!(
            (p.text.as_str(), range(&p)),
            ("calendar", (date!(2023 - 01 - 01), date!(2023 - 12 - 31)))
        );
    }

    #[test]
    fn skipping_a_kind_gives_the_words_back() {
        let q = "dog in Lisbon june 2023";
        let p = parse(
            q,
            TODAY,
            &places(),
            Skip {
                date: true,
                ..Skip::default()
            },
        );
        assert!(p.date.is_none());
        assert_eq!(p.place.as_ref().unwrap().value, "Lisbon");
        assert_eq!(p.text, "dog june 2023");
        let p = parse(
            q,
            TODAY,
            &places(),
            Skip {
                place: true,
                ..Skip::default()
            },
        );
        assert!(p.place.is_none());
        assert_eq!(p.text, "dog in Lisbon");
        let p = parse(
            q,
            TODAY,
            &places(),
            Skip {
                date: true,
                place: true,
                ..Skip::default()
            },
        );
        assert_eq!(p.text, q);
        assert_eq!(
            Skip::parse(Some("date, PLACE")),
            Skip {
                date: true,
                place: true,
                people: false
            }
        );
        assert!(Skip::parse(Some("people")).people);
        assert_eq!(Skip::parse(Some("nonsense")), Skip::default());
        assert_eq!(Skip::parse(None), Skip::default());
    }

    fn named(names: &[&str]) -> Vec<Named> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| Named {
                id: Uuid::from_u128(i as u128 + 1),
                name: (*n).into(),
            })
            .collect()
    }

    fn with(q: &str, names: &[&str]) -> Parsed {
        parse_with(q, TODAY, &places(), &named(names), Skip::default())
    }

    fn who(p: &Parsed) -> Vec<&str> {
        p.people.iter().map(|x| x.name.as_str()).collect()
    }

    #[test]
    fn people_lead_trail_or_follow_an_introducing_word() {
        let names = ["Person A", "Person B"];
        let p = with("Person A beach", &names);
        assert_eq!((who(&p), p.text.as_str()), (vec!["Person A"], "beach"));
        let p = with("beach Person A", &names);
        assert_eq!((who(&p), p.text.as_str()), (vec!["Person A"], "beach"));
        let p = with("beach with Person A at dusk", &names);
        assert_eq!((who(&p), p.text.as_str()), (vec!["Person A"], "beach at dusk"));
        assert_eq!(p.people[0].matched, "with Person A");
        // In the middle of the query, with nothing to introduce it: left alone.
        let p = with("beach Person A dusk", &names);
        assert!(p.people.is_empty());
        assert_eq!(p.text, "beach Person A dusk");
    }

    #[test]
    fn several_people_and_a_date_make_a_filter_only_search() {
        let names = ["Person A", "Person B"];
        let p = with("Person A and Person B 2023", &names);
        assert_eq!(who(&p), vec!["Person A", "Person B"]);
        assert_eq!(p.text, "");
        assert_eq!(range(&p).0, date!(2023 - 01 - 01));
        let p = with("photos of Person B with Person A", &names);
        assert_eq!(who(&p), vec!["Person B", "Person A"]);
        assert_eq!(p.text, "");
        // Next to each other, in either order of discovery.
        let p = with("beach Person A Person B", &names);
        assert_eq!((who(&p), p.text.as_str()), (vec!["Person A", "Person B"], "beach"));
        // The same person twice is one filter.
        assert_eq!(who(&with("Person A and Person A", &names)), vec!["Person A"]);
    }

    #[test]
    fn the_longest_name_wins_and_a_shared_name_is_never_guessed() {
        let p = with("Ann Lee beach", &["Ann", "Ann Lee"]);
        assert_eq!((who(&p), p.text.as_str()), (vec!["Ann Lee"], "beach"));
        let p = with("Ann beach", &["Ann", "Ann Lee"]);
        assert_eq!(who(&p), vec!["Ann"]);
        // Two people called Anna: "Anna" is nobody in particular, and "Anna Lee" is not read as "Anna" and a leftover.
        let p = with("Anna beach", &["Anna", "Anna"]);
        assert!(p.people.is_empty());
        assert_eq!(p.text, "Anna beach");
        let p = with("Anna Lee beach", &["Anna", "Anna", "Anna Lee"]);
        assert_eq!(who(&p), vec!["Anna Lee"]);
        let p = with("Anna Lee beach", &["Anna", "Anna", "Anna Lee", "Anna Lee"]);
        assert!(p.people.is_empty());
    }

    #[test]
    fn names_that_are_ordinary_words_need_an_introducing_word() {
        let p = with("rose garden", &["Rose"]);
        assert!(p.people.is_empty(), "rose garden is a garden");
        let p = with("garden rose", &["Rose"]);
        assert!(p.people.is_empty(), "a trailing common word is not a person");
        let p = with("garden with Rose", &["Rose"]);
        assert_eq!((who(&p), p.text.as_str()), (vec!["Rose"], "garden"));
        // Months and the places' own ambiguous words are common too.
        assert!(with("may beach", &["May"]).people.is_empty());
        assert_eq!(who(&with("beach with May", &["May"])), vec!["May"]);
        assert!(with("hope", &["Hope"]).people.is_empty());
        // Short names are never matched.
        assert!(with("Al beach", &["Al"]).people.is_empty());
        // A name that is not common matches at the start.
        assert_eq!(who(&with("Maria sunset", &["Maria"])), vec!["Maria"]);
    }

    #[test]
    fn a_place_or_date_read_first_keeps_its_words_and_skip_turns_people_off() {
        // "in Lisbon" is a place before it is a person's name.
        let p = with("dinner in Lisbon", &["Lisbon"]);
        assert_eq!(p.place.as_ref().unwrap().value, "Lisbon");
        assert!(p.people.is_empty());
        let q = "Person A beach";
        let off = parse_with(
            q,
            TODAY,
            &places(),
            &named(&["Person A"]),
            Skip {
                people: true,
                ..Skip::default()
            },
        );
        assert!(off.people.is_empty());
        assert_eq!(off.text, q);
        // No people known (People is off, or nobody is named): nothing is read.
        assert!(parse_with(q, TODAY, &places(), &[], Skip::default()).people.is_empty());
    }

    #[test]
    fn at_most_ten_people_are_read() {
        let names: Vec<String> = (0..12).map(|i| format!("Person {}", (b'A' + i) as char)).collect();
        let list: Vec<&str> = names.iter().map(String::as_str).collect();
        let q = names.join(" and ");
        let p = with(&q, &list);
        assert_eq!(p.people.len(), MAX_PEOPLE);
    }

    #[test]
    fn odd_input_does_not_panic() {
        for q in [
            "",
            "   ",
            "in",
            "in in in",
            "- - -",
            "2023-13",
            "2023-02-30",
            "0000",
            "june 31 2023",
            "june 5",
            "5 june",
            "of 2023",
            "ünïcode 2023",
            "犬 in Lisbon",
        ] {
            let _ = run(q);
        }
        assert!(run("2023-13").date.is_none());
        assert!(run("2023-02-30").date.is_none());
        // 30 days in June: not a day, and what is left of it is just the year.
        assert_eq!(run("june 31 2023").date.unwrap().label, "2023");
        assert_eq!(run("犬 in Lisbon").text, "犬");
    }
}
