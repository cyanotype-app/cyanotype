# Faces in photos

The worker can find the faces in every photo and keep a fingerprint (an *embedding*, 128 numbers) of each, and group the fingerprints into
people that the owner names and corrects: a People page, a page for each person, the people in a photo's Info panel, "Is this X?" suggestions and
"Not this person" and "Not a face". It finds groups to propose; it never claims to know who anyone is.

**It is off by default.** `FACES_ENABLED=1` turns it on (worker and assets service); leave it unset and no job is queued, no model is read, nothing
is stored, the People routes of the API answer `404` as if they did not exist and the web app shows no People entry anywhere. A face fingerprint is biometric data about the people in a library, some of whom never agreed to anything: read
"Privacy" below before switching it on.

## Contents

- [What it does](#what-it-does)
- [People: grouping, naming and fixing](#people-grouping-naming-and-fixing)
- [People in search, albums and bulk actions](#people-in-search-albums-and-bulk-actions)
- [Privacy](#privacy)
- [Models, licences and provenance](#models-licences-and-provenance)
- [Bake-off](#bake-off)
- [Settings](#settings)
- [Cost: time and memory](#cost-time-and-memory)
- [Operating it](#operating-it)
- [Tests](#tests)

## What it does

1. After a photo's `embed` job (search indexing) finishes, a `faces` job is queued for it at backfill priority. When the feature is switched on
   on an existing library, the worker's sweep (every 15 minutes) queues one for every photo that has a preview and has not been scanned. That
   is the backfill. The sweep also covers a model switch and lost jobs.
2. The job runs in a loop of its own, after everything else: it takes a job only when no thumbnail, metadata, embed, text or other
   foreground job is runnable, and it shares the embed loop's one-at-a-time permit, so the search model, the text model and the face models never
   work at the same moment. The models load on the first job and are dropped after five quiet minutes, like the others.
3. It reads the photo's **preview** (`preview.webp`, at most 1440 px on the long edge), never the original, so there is no large decode
   and nothing to refuse for memory. While it runs it holds 96 MB of the worker's byte budget (`WORKER_MEMORY_BUDGET_MB`).
4. YuNet finds the faces and five landmarks each (eyes, nose, mouth corners). Each face that is at least 32 px (shorter side of its box,
   in the preview) is aligned to the 112 x 112 ArcFace template (a similarity transform of its landmarks; code and unit tests in
   `crates/cyanotype-faces/src/align.rs`) and SFace turns the crop into a unit-length 128-dimension fingerprint. Smaller faces are stored
   without one (a guess at 20 px would only join strangers).
5. The result replaces the photo's rows for this model in one transaction (`faces`) and records that the photo was scanned, also when it has no
   faces (`face_scans`), so running the job again never duplicates a face. What the owner decided about the photo's earlier faces (their person,
   rejections, a chosen cover) is carried over to the new ones, see below.

### Is the preview enough?

For people it is, with one limit worth knowing. A face has to be at least 32 px in the *preview* to get a fingerprint, and the preview is 1440 px
on its long edge: a face needs a shorter side of about 2.2 % of the photo's long edge. Smaller faces (people far away, big crowds) are found
without a fingerprint or missed. A pass over the original (a 24 MP decode is 100 MB or more, and the budget is 640 MB shared with thumbnails)
would reach smaller faces and was not worth the memory risk for a first version; a face that small is also the one the recognizer is least sure
about (the table below). If the People
page needs it, the way is a second, optional job that decodes the original in strips, not a bigger preview.

## People: grouping, naming and fixing

Migration 0030 adds `people` (owner, optional name, hidden, optional cover face), `face_rejections` (face, person: "this face is not that
person"), `people_runs` (the request and state of a grouping run, one row per owner), and gives `faces.person_id` its foreign key (a deleted
person frees its faces) and `faces.assigned_by` (`auto` or `user`).

### The grouping run (worker)

A run is per owner. It is not a row of the job queue (those are per photo) but follows the near-duplicate scan: the faces job asks for a run when it
stores faces (`people_runs.requested_at`), and the loop in `apps/worker/src/people.rs` takes an owner when **none of that owner's photos has a
`faces` job queued or running and the last request is two minutes old** (`QUIET_SECS`), so a backfill of thousands of photos groups once, and
only when no thumbnail, metadata, embed, text or other foreground job is runnable. A request made while a run is going stays wanted. On start and
hourly, owners with fingerprinted faces and no run on record (faces stored before People existed) are asked once. A run does this, in order:

1. **Joining people already known.** Every unassigned, fingerprinted face of a live photo asks the model's HNSW index (the same expression and
   predicate as the migration, with the search's iterative-scan settings, because most of the index is other owners' and assigned faces) for its 12
   nearest faces that have a person. It joins the person of the single most similar one when that is at least **0.60**, the next person is at least
   **0.08** behind, and the face was not rejected from that person. All decisions are made against the people as they were when the run began (a
   face that joins now is not an anchor for the next), then applied.
2. **Grouping the rest.** The still unassigned faces get a 12-nearest-neighbours graph among themselves; edges are pairs at least **0.55** alike. Edges
   are taken strongest first, and two groups join only if **no photo is in both** (two faces in one photo are two people) and the cosine of their
   centres (the mean fingerprint of each) is at least **0.50**: a single link joins strangers' chains through look-alikes, a centre does not.
3. **What a group becomes.** A group of two or more faces whose centre is at least **0.55** from exactly one known person (none of its faces
   rejected from them) joins that person; if two people are that near it is left alone. Otherwise a group of at least **3 faces from at least 2
   photos** becomes a new unnamed person. Smaller or uncertain groups stay unassigned until they grow.
4. Unnamed people left without any face are removed. **Named people are never deleted or merged automatically.**

**The owner's decisions always win.** A face the owner put in a person (`assigned_by = 'user'`) is never moved; a face is never assigned to a person it
was rejected from; every write of a run is guarded in SQL (`person_id IS NULL AND NOT EXISTS (rejection)`), so a decision made while a run is going
is not overwritten. Faces without a fingerprint (under 32 px) are never grouped. Automatic assignments are not revisited: a wrong one is fixed with
"Not this person", which also stops it coming back.

### A new scan keeps the decisions

`faces::store` matches each new face of a photo to one of its earlier faces (one to one, strongest first): first by fingerprint (cosine at least 0.363, the
same-person threshold, only when both come from the same model: a rotate, a crop or a new version of the model), then by box overlap (IoU at least 0.5: a face
without a fingerprint, or another model). The new face takes over the person and who decided, the rejections and being a cover. After a rotate or crop
the photo's scan is forgotten, and the faces the owner decided about wait, unread (every reader asks for a photo with a scan), until the new scan finds
them again. A face marked "Not a face" (below) is carried over the same way. Faces of another model keep their rows until the sweep prunes them, but give up what moved on, so a person is counted once. Tests:
`crates/cyanotype-core/tests/people.rs` and the faces job test in `apps/worker/src/faces.rs`.

### How the thresholds were chosen

`cargo test --release -p cyanotype-faces --test bakeoff clustering -- --ignored --nocapture` (the evaluation set above, the largest face of each of 130 photos
of 22 people; setup as for the bake-off). The set is small and every face is in its own photo, so it measures how far apart fingerprints are, not
how a library behaves. The numbers that matter, and the reasoning that goes beyond them:

| cosine at least | genuine pairs kept | impostor pairs (of 8,049) | chance that one of M strangers' faces is this alike (tail fitted to the top 5 %) M=1,000 / 10,000 |
| --- | --- | --- | --- |
| 0.40 | 92.0 % | 1 | 36 % / 99 % |
| 0.45 | 86.3 % | 0 | 10 % / 63 % |
| 0.50 | 78.6 % | 0 | 2 % / 20 % |
| 0.55 | 69.0 % | 0 | 0.5 % / 5 % |
| 0.60 | 58.6 % | 0 | 0.1 % / 1 % |

- **Why not 0.363.** That figure is per pair of photos (0.1 % of pairs). A new face is compared with *all* the faces of everyone already known, so
  what matters is the chance that any of thousands looks like a stranger: the table's last columns. A false join costs the owner's trust, a missed one
  costs a click, so the numbers lean to splitting.
- **Joining a person (0.60, margin 0.08).** A new face against everyone else's faces, leave one out: at 0.60, 80.8 % went to the right person, none to a wrong one and
  19.2 % stayed alone (they are grouped later or suggested); at 0.55, 90.8 % right and none wrong; at 0.50, 95.4 % and none wrong. 0.60 was chosen over what the set
  supports because the tail estimate says 0.55 would make about one in twenty faces meet a stranger at least that alike once a library has 10,000 known faces.
- **Grouping (link 0.55, centres 0.50).** Single-link components of the set are 100 % pure from 0.45 up (91.6 % at 0.40, with one group of 11 made of two people) but
  chain strangers in a big library; at link 0.55 the grouping is 100 % pure with 82.1 % pair recall, 35 groups for the 22 people (20 of them of 3 or more faces:
  people split in two rather than joined), and in the shipped rule 20 of 22 people have a group of their own. The centres of two halves of one person's faces are at
  least 0.574 apart, those of two different people at most 0.450, which is where the 0.50 and 0.55 for groups come from.
- **Suggestions (0.45 to the centre of the person).** A face against the centre of a person's other faces, for every face and person: at 0.45 every suggestion was right and
  98.5 % of the right ones were made; 0.40 is also 100 % precise (99.2 %), 0.35 only 90.9 %. The owner answers each, so this is lower than the automatic threshold,
  but a stranger in the list is an annoyance, so it stays at 0.45.

Not measured: children, profiles, masks, ageing over decades inside one person, siblings and twins, and any demographic difference. Those make people split (a miss) and
look-alikes join (the risk the margin and the high thresholds exist for); the owner's tools below are the answer to both.

### Cost of a run

Measured on 30,000 synthetic fingerprints (1,200 identities, 25 faces each, one owner; `cargo test --release -p cyanotype-core --test people big_library -- --ignored
--nocapture`, shared 4-core machine, Postgres in a container): the first run, nothing known, **76 s** (all 30,000 faces grouped into 1,200 people); a later run for 1,500
new faces against 1,200 known people **5 s** (about 3 ms a face). That is one nearest-neighbour query per face against the HNSW index and about 30,000 x 12 edges
in memory (a few MB) plus the fingerprints (15 MB per 30,000 faces): **for 50,000 photos expect two to three minutes for the first run and seconds afterwards**, in the
background and behind everything else. Synthetic identities are easier than real ones (real runs leave more faces unassigned), but the cost is the queries, not the
data. The run reads and writes only the database: no model, no photo, none of the worker's byte budget.

### What the owner can do

All of it is the owner's own session (or API key) and nothing else:

| | |
| --- | --- |
| `GET /assets/people/status` | `{enabled}`: the only People route that answers while the feature is off. |
| `GET /assets/people`, `GET /assets/people/{id}` | `{id, name, hidden, faces, photos, pending, cover}`; `cover` is `{face_id, asset_id, box, preview_url}` (the box is fractions of the preview; the web app crops it with CSS, no new file). Counts and covers are of the photos this session may see (not trashed, not hidden unless the Hidden view is unlocked); a person only hidden photos have is not listed to a locked session. `pending` is how many "Is this X?" suggestions wait for a named, shown person (0 for the others): counted for all the people in **one** query (the centre of each person's faces is an `avg`, each asked the index through a lateral join, same rules and the same 300 nearest faces as the suggestions), so the list does not cost a query per person. |
| `PATCH /assets/people/{id}` | `{name?, hidden?, cover_face_id?}`: rename (null or blank clears), hide, cover. |
| `DELETE /assets/people/{id}` | The person goes, its faces are free (they may be grouped again; to keep someone out of sight, hide them). |
| `POST /assets/people/{id}/merge {into}` | All faces to the other person, which keeps its name unless it has none; "not the first" becomes "not the second". Audited as a count only. |
| `POST /assets/people/bulk {action, ids, into?, name?}` | Several people at once, in one transaction and owner-checked (an id that is not the caller's is a `404` and nothing changes): `action` is `hide`, `show`, `delete` (the same as the single ones; the photos stay) or `merge` (folds `ids` into `into`, one of them, with the single merge's rules; the first name found is taken over if `into` has none; `name`, if sent, is what the person ends up called). Audited as counts only, one entry per merged or deleted person. |
| `GET /assets/people/{id}/faces?limit&cursor` | The faces a cover can be chosen from (`{face_id, asset_id, box, preview_url}`, largest and most confident first, in photos this session may see), `next_cursor` while there are more: the picker of **Change cover**. |
| `GET /assets/people/{id}/photos?limit&cursor` | The person's photos, same items and keyset paging as `GET /assets`. |
| `POST /assets/people/{id}/faces/remove {asset_ids?, face_ids?}` | "Not this person": the faces become unassigned and a rejection is written. |
| `GET /assets/people/{id}/suggestions` | "Is this X?": unassigned faces at least 0.45 like the person's centre, best first, none rejected. |
| `POST /assets/faces/{id}/assign {person_id}` or `{new: true, name?}`, `POST /assets/faces/{id}/reject {person_id}` | Accept a suggestion or name a face; refuse one. Moving a face from a person rejects it from them. |
| `POST /assets/faces/{id}/not-a-face`, `DELETE` the same | "Not a face" and its undo, see below. `POST` answers `{person_id}`, the person the detection was in (or null). |
| `GET /assets/{id}/faces` | `{items, dismissed}`: the faces in one photo and their people (the faces drawn on the photo in the viewer, and the named people of its Info panel). Faces of hidden people are left out of `items`, and so are detections marked not a face, which are in `dismissed` (`[{id, box}]`) so they can be brought back. |

Every change asks for a new run. Key scopes: reads need a read key, changes a write key (`key_scope_tests`). The web app: **People** in the library's More menu (only when the
feature is on) opens a grid of round faces with name and counts in three sections that are never mixed: the named people, then **Unnamed** (open one to name it), then **Hidden people**, collapsed. A named
person with "Is this X?" suggestions waiting wears a count in the corner of the card ("3 photos to confirm" to a screen reader; nothing at 0, "99+" above that); it is read again
when the page opens, so answering on the person's page updates it. **Select** on the page picks several people for **Merge** (choose who stays and optionally type the name they end
up with, then confirm), **Hide** / **Show** and **Delete** (after asking: the person and the name go, the photos stay). A person's page has the photos in the usual grid and
selection, a round face and the photo count at the top (tap the face for **Change cover**), and a **⋯** menu with Rename (a sheet; blank clears the name), **Change cover** (a paged picker of the person's faces, usable with the keyboard and a screen reader), Hide, Save as album, Search with filters, Merge into, Delete person, selected photos to **Not this person** or **Use as cover**, and "Is this X?" with Yes and No; the Info panel of a
photo lists its named people (see "Faces on the photo"). **Only named, shown people are ever a choice in a menu or picker**: the merge picker, "Who is this?", the
search filter, the names read out of a typed query and the smart-album rule (the server's `named_people` for the query parser and `filterablePeople` in the web app); the People page itself is the one
place unnamed and hidden people are listed. Metrics (`docs/observability.md`): `cyanotype.people.run_ms`, `.assigned`, `.created`,
`.ungrouped_faces`.

### Not a face

A detector finds the odd pattern, reflection or poster that is no face at all. **Not a face** marks such a detection (migration 0031: `faces.not_face`, with a
check that a marked face has no person). It is not "Not this person": that one says whose face it is not and the face goes on being grouped and suggested to
others, this one says there is no face, so the detection is out of everything:

- the grouping run neither groups it nor joins it to a person (`load_ungrouped`, the guarded writes and the first-run request all skip it), and "Is this X?"
  never suggests it;
- every count leaves it out: a person's faces and photos (marking takes the face out of its person, without writing a rejection), the ungrouped faces of the run
  report and the `people.ungrouped_faces` metric, and the suggestions of a person. The scan's own `face_scans.faces` ("faces found") is the detector's count and
  stays as it was;
- a new scan of the photo (a rotate or crop, a new model version) hands the mark to the face that takes the old one's place, like a rejection, and a rotate or
  crop keeps the marked rows while it waits for that scan;
- it is undone by `DELETE /assets/faces/{id}/not-a-face` (the face is an ordinary unclaimed one again, and may be grouped), by **Undo** in the toast that follows the
  mark (which also puts a face that was in a person back in them), and by **Bring back** in the photo's Info panel (all of the photo's marked detections at once). An
  unnamed person left without faces by a mark is removed, like when a face is moved; a named one stays.

It is offered in "Who is this?", in "Is this X?" and, for a named face, as a small button that shows with the face's outline in the viewer. Owner-only and off with
the feature like the rest: nothing about it is in a share, a public response or a recovery manifest.

### Faces on the photo

In the viewer (the owner's own session, People on) the faces of the photo are regions over the picture, drawn in the zoomable layer like the Text panel's boxes, so
they sit on the faces at fit and follow every zoom and pan. A region is a link for a face that has a named person, which opens that person's page, and a button for
any other face, which opens "Who is this?"; both are reached with Tab, outlined on hover and when focused, labelled for a screen reader ("Person A: open their page",
"Unknown face: who is this?"), and open with Enter or Space. Faces of hidden people and detections marked not a face are not drawn. A tap or click is heard by the
viewer's own zoom handler, not by the region (the photo area owns every pointer): a press that pans or pinches never opens anything, and a tap on a face waits out the
double-tap window (about a third of a second) so that a double tap or double click zooms instead of opening it. The Info panel's **People** row lists only the people
who have a name, once each, and has the switch **Show face boxes on the photo** (outline every face all the time) and, when there are any, **Bring back** for the
detections marked not a face.

"Who is this?" and "Is this X?" show the whole photo (its preview) with the face in question outlined and the rest dimmed, so it is clear which face of a group photo is
meant. "Is this X?" is on a person's page: the face's crop opens it ("See the whole photo") and it has Yes, No and Not a face next to the crop's own Yes and No.

## People in search, albums and bulk actions

A person is one more way into the library. Everything here follows `FACES_ENABLED` and the owner's own people, and none of it is ever visible to
anyone but the owner (see "Privacy": a share link made from a person carries photos, never people).

### The person's page is a timeline

`GET /assets/people/{id}/timeline` is `{total, days: [{day, count}]}`, the person's photos per day, and `GET /assets/people/{id}/photos` also takes
`before` (as `GET /assets` does), so the page is the library's own timeline component over the person's endpoints: month and day headers, the
scrubber, "Select all" without listing ids. The photos are the ones the person's page has always listed (live, **archived ones too**, hidden ones
only with the Hidden view unlocked). The page reads the person's counts every 15 s while it is open and lays the timeline out again when they
change (the worker grouped new photos), unless a selection is open (it waits for the end of the selection).

### The `{"person": id}` scope

A bulk request (`favorite`, `rating`, `tags/apply`, `hide`, `archive`, `trash`, `restore`, `edits`, albums, `shares`) takes
`{"scope": {"person": "<id>"}, "except": [...]}` like it takes `{"album": id}` or `{"search": ...}`: the server resolves the photos inside the
request's transaction, as the person's page lists them. An id that is not the owner's, or one that never existed, is `404` (the same), and so is
every `person` scope while People is off. A `search` scope with a `people` filter is **refused (404) while People is off**, never read as "no filter":
a bulk trash must not widen to the whole library because a switch was flipped.

### "Save as album"

A person's page (the header, or the selection bar for a part of it) and a filter-only search with people in it offer **Save as album**: the album is
created empty (`POST /assets/albums {title}`) and then filled by scope (`POST /assets/albums/{id}/assets {scope: {person}}` or
`{scope: {search: {people, from, ...}}}`), so a person with 4,000 photos is one request and the browser never lists ids. The album is an ordinary
one, a snapshot: it does not follow the person afterwards (a smart album with a People rule does). **The title starts empty and the dialog says
why**: an album's title is shown to everyone it is shared with and is written into the recovery manifest, so it must not be a person's name
by default.

### The People filter of search

`GET /assets/search?people=<id>,<id>` (also `similar` and the `search` scope; at most 10 ids): photos where **all** of the people are, **together** in the
same photo (one `IN (SELECT asset_id FROM faces ...)` test per person, through `faces_person_idx`, joined to the owner's `people` and to the face's own scan
like the person's page, so the filter and the page agree). An id that is not the owner's matches nothing, exactly like an id that never existed (the
answers are identical); a malformed id or more than 10 is `400`. While People is off the parameter is ignored. In the web app: a **People** group in the
Filters sheet (named people who are not hidden, a box to narrow the list, "Photos with all of the chosen people in them"), shown as one chip
("Person A and Person B"), in the address as `people=`, combinable with everything else. The person's page has **Search with filters**, which is how to
get "Person A in 2023" and save it as an album.

**Names in the words of a query.** "Person A and Person B 2023", "beach with Person A", "Person A beach": the parser reads the owner's **named, not
hidden** people with a photo a search can show (the list is read per query, never cached: a rename or a hide counts at once), after dates and
places (so "in Lisbon" stays a place), with these rules (`query_parse.rs`):

- A name counts at the **start** or the **end** of the query, or right after **with, of, and, featuring, plus, including** ("beach with Person A",
  "Person A and Person B"). In the middle of a query with nothing to introduce it, it is just words.
- The **longest name wins** ("Alpha Bravo" over "Alpha"). A name that **two people share is never matched** (and "Delta Echo" is not read as "Delta" plus
  a leftover), names of **fewer than three letters** are not matched, and the same person named twice is one filter (at most 10).
- A single word that is also an ordinary word, a month or season, a preposition, filler or a place the library often has (`COMMON_NAMES`,
  `AMBIGUOUS`: will, rose, grace, may, hope, dog, mom...) counts **only after an introducing word**: "with Sky" is a person, "sky garden" is a garden.
- What was understood is a removable chip (`parsed.chips[].kind = "people"` with `people: [{id, name}]`); its x searches again with `skip=people`,
  which gives the words back. An explicit `people` filter wins over the words (they stay in the query). While People is off nothing is read.
- False positives are possible by nature (a leading first name that is also a book title); the chip says what was understood and one click undoes it.

### Smart albums

Rule `{"field": "people", "ids": [...]}`: **all of these people are in the photo** (1 to 10), in the rule editor as a *People* rule (offered only while People is
on). Saving checks every id is the owner's (`404` otherwise, the same as an unknown id) and that People is on (`404` while it is off). The album stores only
ids, so a rename changes nothing, a deleted person leaves a rule that matches **nothing** (never "everything"), and a merge is not followed (edit the rule). While
People is off a saved people rule matches nothing, in the album, its counts, its bulk scope and its share link. Because ids are the owner's private data, the
**recovery manifest leaves the rules out of a smart album that has a people rule** (`manifest::has_people_rule`): `reindex` restores it as an empty album with
its title. The rules are in the owner's own album responses (they are an owner-only API); public share responses never carry them.

### Cost

The filter is one semi-join per person on `faces_person_idx`: with a person's few thousand faces it is the same shape as a tag filter (measure of "Together" below).
"Together" on a very large library intersects the persons' photo sets (hash semi-join of the smaller one against `assets`), so it is as fast as the rarest
person is rare; the planner may instead walk the timeline index newest first and test each photo when nobody is very rare, which is the plan a tag filter
has too. The parser's one extra query per text search reads the owner's named people (`people_owner_idx`, one `EXISTS` on `faces_person_idx` each).

## Privacy

- **Off by default** (`FACES_ENABLED`, in `.env.prod`). Switch it on deliberately.
- **Data stays in your database.** `faces` and `face_scans` (migration 0029): box and landmarks as fractions of the preview, a score, the fingerprint
  (`halfvec(128)`); `people`, `face_rejections` and `people_runs` (migration 0030); `faces.not_face` (migration 0031). **A name you give a person is personal data too**, about the person and
  about you: it is stored in `people.name`, shown only in your own session, and never written to the bucket, the audit trail or a log. No model is called
  over the network: the models are baked into the worker image and run in the worker process.
- **Deleted with the photo.** A permanent delete (`apps/assets/src/purge.rs`) removes the photo's face rows in the same transaction as
  the photo; the foreign keys cascade from `assets` and from `users` too, so deleting an account removes its faces.
  A rotate or crop forgets the photo's scan and the faces nobody decided about (their positions are wrong for the new picture) and scans it again; faces you put in a person wait for the new scan to hand them over. A person you named stays when its photos are purged; an unnamed one left without a face goes.
- **Owner-only, never shared.** Share links, the public pages, downloads, the recovery manifests (`manifest/`) and the CLI import never include faces,
  people or names (a test creates a share of a photo with a named person and searches every public response; another makes an album from a person with
  "Save as album", shares it, and searches every public response, the owner's album and share lists and the recovery manifest for names, person ids and
  face ids). The People routes are for the signed-in
  owner (`404` for anyone else's ids and while the feature is off), and a photo in the Hidden view (or a person only hidden photos have) is invisible to a session that has
  not unlocked it. A recovery from the bucket (`cyanotype reindex`) does not bring faces or people back: **names and corrections are not in the
  recovery manifests either**, so they are lost with the database if it is lost and not backed up (the Postgres dumps do hold them). Faces are found again from the previews when the
  feature is on.
- **One command removes everything.** `cyanotype faces delete [--email ADDRESS] --apply` deletes every face, every person (names included), every "not this
  person" decision, every scan record and every queued or finished `faces` job, for one account or for all. The dry run counts faces, people, decisions, scan
  records and jobs. Deleting an account removes its faces and people with it, a permanent delete of a photo removes its faces and their decisions. It is a dry run (counts only) without `--apply`; it touches nothing else.
  Switch `FACES_ENABLED` off and restart the worker first, or the sweep starts scanning again. Run it where `migrate` runs
  (`docs/deploy.md`: `dc run --rm reindex faces delete --apply`, the one-shot that runs the CLI image).
- **Owner-only progress.** `GET /assets/processing` has a `faces` entry (photos scanned, faces found, queued, running, failed, percent) for the
  caller's own library, only while the feature is on.
- Face recognition has known weaknesses across skin tones, ages and photo conditions (see the limits below). Treat the fingerprints as a way to
  propose groups for a person to confirm, never as an identification.

## Models, licences and provenance

The rule: only models whose weights may be redistributed inside our Docker images. Every licence was read from the model repository's own
files at the pinned commit.

| Model | Used for | Licence | Source (primary) |
| --- | --- | --- | --- |
| **YuNet** (`face_detection_yunet_2026may.onnx`, 0.2 MB, dynamic input size) | finding faces and five landmarks | **MIT** (Shiqi Yu): "All files in this directory are licensed under MIT License" | `opencv/opencv_zoo` commit `26cc381e4d2594bb9f47a26eb8fd96c94a13660d` (2026-05-22), `models/face_detection_yunet/{README.md,LICENSE}`; weights through the repository's Git LFS (the pointer's oid is the sha256 pinned in `models/faces.lock`) |
| **SFace** (`face_recognition_sface_2021dec.onnx`, 37 MB, 128 dimensions) | fingerprints | **Apache 2.0** (Yaoyao Zhong): "All files in this directory are licensed under Apache 2.0 License" | `huggingface.co/opencv/face_recognition_sface` at `3d7082438a6e4551e840c9b2bb60b71e8da4b524` (the OpenCV Zoo's own mirror, 2025-06-20), `README.md` and `LICENSE` |

Both are pinned by exact URL and sha256 in `models/faces.lock`, fetched by `scripts/fetch-models.sh`, verified, and installed with their licence texts
(`LICENSE-yunet.txt`, `LICENSE-sface.txt`) in the worker image under `/opt/cyanotype/models/faces/`, as both licences require. Both are older than 14 days.

**A caveat the licence files cannot settle.** The licences cover the weights and files in those directories. The networks were trained on public
research datasets (YuNet on WIDER FACE; SFace on CASIA-WebFace, VGGFace2 or MS-Celeb-1M, the SFace repository says all three were used for its paper's
experiments, and does not say which produced the OpenCV Zoo's file), and those datasets have their own terms, which we cannot verify or grant. The
OpenCV Zoo licenses the weights it ships under MIT and Apache 2.0 and OpenCV ships them in products; if that residual risk matters for your use,
read the datasets' terms before enabling the feature in a commercial setting.

**Considered and excluded**

| Candidate | Why not |
| --- | --- |
| InsightFace pretrained models (`buffalo_l`, `buffalo_s`, `antelope`, SCRFD, ArcFace R50/R100, ...) | Excluded by rule: "The training data containing the annotation (and the models trained with these data) are available for non-commercial research purposes only", and the same for the downloaded models (`deepinsight/insightface` README, "License"). The code is MIT; the weights are not. |
| ONNX Model Zoo ArcFace (`arcfaceresnet100-8`, `-11-int8`) | The repository is Apache 2.0, but the weights are InsightFace's models trained on refined MS-Celeb-1M (the README names it): the weights' own terms are unclear, so excluded. Also 249 MB (63 MB int8), too big for the worker. |
| SFace int8 (same repository and licence as SFace) | Measured below: slower here (90 ms against 25 ms a face) and less accurate. |

## Bake-off

### The evaluation set

No face-verification set with identity labels that clearly permits redistribution and this use was found: the usual ones (LFW, CelebA, VGGFace2,
CASIA-WebFace, IJB, UTKFace) are research-only, have no licence statement, or are collections of photos from news and Flickr whose copyright
stays with the photographers. So the set is self-assembled from Wikimedia Commons files that the Commons page itself marks **Public domain**
(160 files, mostly works of the US federal government, and photographs old enough to be out of copyright) or **CC0** (6 files):

- `crates/cyanotype-faces/tests/eval/faces.tsv` lists 130 photos of 22 public figures (4 to 8 each; the person column is an anonymous label), with
  the Commons page id (no titles or URLs, which contain names) and licence of each. The files are the **original uploads** (not thumbnails), pinned by sha256;
  `scripts/fetch-face-eval-set.sh [DEST]` looks each page id up on the Commons API, then downloads and verifies the files (default `models/face-eval`, 120 MB).
- How it was made: Commons categories of about 60 people were walked (files whose title contains the person's name and whose licence field is
  public-domain or CC0), the candidates were run through YuNet, and photos with one clear, large face were kept. **The category labels turned
  out to be wrong for about one photo in five** (a namesake, a relative, a coin, a caricature, a painting), so every photo was looked at on contact
  sheets and the ones that were not the person were removed (by eye, not by any model's score), together with four people left with fewer than
  four good photos. 36 of 166 were removed this way.
- Limits, stated plainly: it is small (336 same-person pairs, 8,049 different-person pairs: at a false-match rate of 1e-3 the threshold rests on the
  8 highest-scoring impostor pairs), it is public figures, mostly adults in portraits, from 1860 to now, many in black and white, and **it says
  nothing about demographic fairness**. Children, profiles, masks and poor light in family photos will do worse. Same-event pairs are easier than
  the age gaps the set also has.
- The small-face test is *synthetic*: each portrait's face region is cut out, shrunk so the face's shorter side is 12 to 96 px, and pasted onto a
  1440 x 1080 canvas of smooth random colour with noise (six per canvas, random places). Ground truth is the box the same detector found on the
  full-size portrait, so it measures how much shrinking costs, not how good the detector is on real crowds.

Run it: `scripts/fetch-face-eval-set.sh models/face-eval`, `scripts/fetch-models.sh` and
`LOCK=models/faces-bakeoff.lock scripts/fetch-models.sh models/faces`, then
`FACES_MODEL_DIR=models/faces FACES_EVAL_DIR=models/face-eval cargo test --release -p cyanotype-faces --test bakeoff -- --ignored --nocapture`.
Time and memory: `cargo run --release -p cyanotype-faces --example bench` (its header says how).

### Verification accuracy

Largest face of each photo, detected by YuNet and aligned by our code, fingerprinted by each recognizer. TAR is the true-accept rate at a fixed
false-match rate (FMR); the threshold is read off the impostor pairs.

| Recognizer | Size | TAR @ FMR 1e-2 | **TAR @ FMR 1e-3** | threshold for 1e-3 | AUC | time per face (1 face, 2 threads) |
| --- | --- | --- | --- | --- | --- | --- |
| **SFace (float)** | 37 MB | 99.1 % | **95.8 %** | cosine 0.349 | 0.9996 | 25 ms |
| SFace (8-bit) | 10 MB | 98.8 % | 94.0 % | cosine 0.367 | 0.9995 | 90 ms |

At OpenCV's published cosine threshold of 0.363 the float model has a TAR of 95.2 % and an FMR of 0.05 % on this set, so the published threshold
carries over. (The same crops fed as BGR instead of RGB give 94.3 %: the channel order matters little to this model; RGB is what OpenCV's own
wrapper does.) The 8-bit model is smaller on disk and slower here (the integer kernels did not pay off on this CPU) and less accurate: float wins on
both counts. On the first, uncleaned version of the set (labels as Commons had them) the same measure read 79.9 % TAR, which is what wrong labels do;
the cleaning is described above.

### Small faces and detector input size

Recall of faces of a given size (shorter side of the box, in pixels) on 1440 x 1080 canvases, by the size the detector's input is scaled to
(long edge; a bigger input costs more time and finds smaller faces):

| face px | 640 | 960 | **1280** | 1440 | 1920 (enlarged) |
| --- | --- | --- | --- | --- | --- |
| 16 | 0 % | 25 % | 64 % | 75 % | 94 % |
| 20 | 6 % | 55 % | 82 % | 94 % | 99 % |
| 24 | 27 % | 73 % | 94 % | 97 % | 99 % |
| **32** | 66 % | 94 % | **98.5 %** | 100 % | 100 % |
| 40 | 85 % | 98 % | 99 % | 100 % | 100 % |
| 48 and up | 94-100 % | 100 % | 100 % | 100 % | 100 % |
| false detections per canvas | 0.01 | 0.01 | 0.02 | 0.04 | 0.06 |
| detector time per canvas (2 threads) | 17 ms | 38 ms | 90 ms | 101 ms | 184 ms |

And how good is the fingerprint of a small face? Share of found faces whose fingerprint is within the 1e-3 threshold (cosine 0.349) of the same
person's fingerprint from the full-size photo: 16 px 88 % (mean cosine 0.48), 20 px 93 % (0.62), 24 px 97 % (0.71), **32 px 98 % (0.82)**, 40 px and up 100 %.
So 32 px is where a fingerprint becomes dependable, and that is the size the settings ask for.

### What was chosen, and the judgement

- **YuNet 2026-05 at 1280 px, SFace float.** 1280 finds 98.5 % of 32 px faces for under 100 ms (one thread); 960 loses 5 % of them for half the time,
  1440 gains 1.5 % for a little more.
- **Is it good enough to switch on? Yes, as an opt-in, with the default off.** On this (hard, age-spanning) set the models separate people almost
  perfectly (AUC 0.9996; 95.8 % of the same-person pairs accepted while 0.1 % of different-person pairs are), the detector finds 98 % of faces at 32 px and up
  with 0.02 false detections per photo, and the whole job fits the memory the worker has (below). That is good enough to *propose* groups of faces for a person
  to review, which is what the next step does. It is **not** good enough to name people unattended, to guarantee that a group holds one person only, or to
  claim it works equally well for everyone: the set is small, one-sided, and cannot show demographic differences. The sweep and the delete command make it
  safe to try and to undo.

## Settings

| Setting | Where | Default | |
| --- | --- | --- | --- |
| `FACES_ENABLED` | worker, assets | `0` | `1`, `true`, `yes`, `on` switch it on. Must be the same for both services. |
| `WORKER_FACES_THREADS` | worker | `1` | Intra-op threads of the two sessions (1 leaves a core of a 2 vCPU host to thumbnails and Postgres; 2 takes about 30 % off the time). |
| `FACES_MODEL_DIR` | worker | `<SEARCH_MODEL_DIR>/faces`, i.e. `/opt/cyanotype/models/faces` | Where `yunet.onnx` and `sface.onnx` are. |

Fixed in code (`apps/worker/src/faces.rs`, `crates/cyanotype-faces`): detector input 1280 px on the long edge (previews are smaller and are not
enlarged), score threshold 0.7, overlap threshold 0.3, at most 64 faces per photo, 32 px minimum for a fingerprint, unload after 300 s idle,
sweep every 900 s.

**Changing the model.** `crates/cyanotype-core/src/faces.rs` has `MODEL` and `VERSION`; `models/faces.lock` pins the files. A new model gets a new
id (and its own HNSW index in a migration, like `0029_faces.sql` does: the index is partial on `model`), a change of how faces are made a higher
`VERSION`; the `faces` job's version is `VERSION`, so a new one must be higher than every version before it. Either scans the library again at low
priority; the previous rows stay until every photo has the new ones, then the worker removes them.

## Cost: time and memory

Measured on a 4-core development machine (shared with other work), release build, one thread, previews of 1440 px, a mix of 20 portraits and 30
photos without faces (so the faces per photo are about 0.7):

| | |
| --- | --- |
| detection, one photo, detector input 1280 | 75 ms (960: 32 ms; 1440: 87 ms) |
| alignment and fingerprint, per face | 27-33 ms |
| whole job per photo (median / p95), without the download and the store | 105 ms / 140 ms at 1280 (63 / 79 ms at 960); with 2 threads 74 / 150 ms |
| loading the two models | about 100 ms |

**Resident memory** (`/proc/self/status`, steady state after processing photos):

| | resident |
| --- | --- |
| SigLIP 2 image tower loaded and used (the search job) | 140 MB |
| + face models loaded | 174 MB (+34 MB) |
| + after scanning photos (activations, allocator slack) | 213 MB (+73 MB over the image tower alone) |

*Answer to "does it fit the 1280m worker next to the SigLIP image tower?"* Yes. The compose file's 1280m is the 640 MB decode budget plus the models
that stay resident plus runtime slack; the face models add about 75 MB when all three loops have their models loaded (search 140, text about 100 per
`docs/build.md`, faces 75: about 315 MB), inside the slack that was left, and a face job's transient memory (the decoded preview, the 1280 px input
tensor of about 15 MB, the activations) is paid for out of the shared budget (96 MB per job). The limit is unchanged; `worker.rss_bytes` and
`faces.model_loaded` show it live.

*Backfill of 50,000 photos.* About 105 ms of model time per photo here, plus the preview's decode, its download and the database write (about 40 ms):
roughly 0.15 s a photo. A shared 2 vCPU VPS with one thread is slower than this machine, by 1.5 to 2 times, and a family library has more faces per photo
than this mix: **plan on 3 to 5 hours** for 50,000 photos at the default one thread (2 to 3 hours with two), in the background behind uploads, search
indexing and text. The queue is a job per photo; `GET /assets/processing` shows `faces.percent`.

## Operating it

- Turn on: set `FACES_ENABLED=1` in `.env.prod`, redeploy (`docs/deploy.md`). Nothing to install: the models are in the worker image.
- Watch: `GET /assets/processing` (`faces`: `scanned`, `total`, `found`, `queued`, `running`, `failed`, `percent`), the Datadog dashboard
  (backlog, photos per minute, time per photo, faces per photo), and the metrics below. Failed `faces` jobs show in the failure list and "Retry
  failed" requeues them.
- Turn off: unset it and restart. Existing data stays until you delete it.
- Delete: `cyanotype faces delete --apply` (above).

Metrics (`docs/observability.md`): `cyanotype.faces.model_loaded` (gauge), `cyanotype.faces.duration` (ms per photo, tag `model`),
`cyanotype.faces.per_photo` (faces found per photo), `cyanotype.faces.model_load_ms`, the `faces` kind of `queue.*` and `jobs.*`.

## Tests

- `crates/cyanotype-faces`: unit tests with no model for the YuNet decoding (cell, box and landmark formulas), non-maximum suppression, the input plan and
  tensor, the similarity transform (a known rotation, scale and shift is recovered; the template maps to itself; degenerate input) and the 112 x 112 warp
  (landmarks land on the template); `tests/model.rs` with the real models on three public-domain portraits (a face is found where it is, landmark order,
  same person closer than another, no face in a gradient, small faces get no fingerprint); `tests/bakeoff.rs` (ignored) is the measurement above.
- `crates/cyanotype-core/tests/faces.rs` (needs `DATABASE_URL`): rows replaced, never duplicated; halfvec stored and searchable through the index, wrong
  dimension refused; nothing stored for a photo being deleted; the sweep; progress; the cascade on photo and account deletion; `delete_all` (dry run, one
  owner, everyone); forgetting after an edit; pruning replaced models.
- `apps/worker/src/faces.rs`: the job against a real database, bucket and models: a portrait gets a face with a fingerprint and running again replaces
  it; skips and failure classes; the sweep and the pruning.
- `apps/assets`: the processing report (absent while off, owner-scoped on) and the purge removing a photo's faces, rejections and orphaned unnamed people;
  `routes/people_tests.rs`: the switch (every People route a `404` while off), the list with counts and covers, owner isolation on every verb, rename/hide/cover
  with bad input, merge and delete, the bulk changes (hide, show, delete, merge with the name rules; all or nothing; another owner's id is a `404`), the cover choices (paged, hidden photos,
  owner), the pending count (right, one query's worth for every person, never from another owner's faces, moves with a Yes and a No), "not this person", assign and reject, suggestions, the Hidden view, a photo's faces, and that no share link carries anything of it.
- `crates/cyanotype-core/tests/people.rs`: a rescan after a rotate, a version change and a model change keeps people, rejections and covers; grouping makes people only
  from enough faces; a new face joins the right person and never one it was rejected from; the owner's decisions win and other owners are invisible; faces of one
  photo are never one person; the queue (quiet period, queued scans, newer requests); suggestions; cascades; `delete_all`; the HNSW index is used. `people_math`
  has the unit tests of the arithmetic. Web: `peopleText.test.ts`, `People.test.tsx`, `PersonPage.test.tsx`, `FaceChips.test.tsx` and `e2e/people-layout.spec.ts`
  (phone widths 320 to 414, desktop, dark).
- People in search, albums and bulk actions: `query_parse.rs` (unit: the name rules above), `routes/people_tests.rs` (the person's timeline and its paging, the `person`
  scope on every bulk verb and its owner isolation, "Save as album" from a person and from a filtered subset, the shared album that leaks nothing, the filter
  alone and together, with dates, favorites, the trash and hidden photos, another owner's id, malformed and too many, names in a query and their chips, smart
  albums with a people rule, and with the switch off: the parameter ignored, the scopes and rule refused, the parser silent, a saved rule matching nothing),
  `manifest.rs` (a people rule stays out). Web: `peopleText.test.ts`, `searchText.test.ts`, `smartText.test.ts`, `PeoplePicker.test.tsx`, `FilterSheet.test.tsx`,
  `SmartAlbumEditor.test.tsx`, `Search.test.tsx`, `PersonPage.test.tsx` (the timeline, live refresh, select all through the scope) and
  `e2e/people-search-layout.spec.ts`.
- Not a face and faces on the photo: `crates/cyanotype-core/tests/people.rs` (a marked detection is never grouped, joined or suggested and is left out of the run's
  numbers; the mark survives a rotate, an edit and a rescan, also by box; undo; another owner's faces), `routes/people_tests.rs` (marking and undoing over the API: the
  person's counts and photos, the cover, an unnamed person's removal, suggestions, `dismissed` in a photo's faces, `404` for someone else's face and while off, assigning
  a marked face). Web: `FaceChips.test.tsx` (both dialogs with the box, Not a face, the regions, the Info row), `PersonPage.test.tsx` ("Is this X?" dialog, Not a face and its
  undo), `e2e/viewer-faces-layout.spec.ts` (regions on the faces at fit, zoom and pan with the mouse and CDP touch, hover, focus and keyboard, a click opening a face but a
  double tap, pinch or pan never doing so, hidden and marked faces absent, the dialog at 320, 375, 414 and 1280 px light and dark) and `e2e/people-layout.spec.ts`.
