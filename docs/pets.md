# Dogs and cats in photos

The worker finds the dogs and cats in every photo, so the owner can list them (a **Pets** view), search for them ("dogs", "cat on the sofa", a
pets filter, a smart-album rule) and select them for bulk actions. A dog or a cat is first just a *pet*: the photos are found, not the animals told apart.
**Named pets** go one step further, seeded by the owner: they make "Biscuit", tag a few photos of it, and the app proposes more photos by how much the crop of
the animal looks like the tagged ones; the owner confirms or refuses ([Named pets](#named-pets)). Nothing groups animals by itself: no open model tells individual
animals apart well enough for that, and the measurement says how well it does tell them apart ([Telling individuals apart](#telling-individuals-apart)).

**It is on by default** and costs very little: the search model's own embedding of a photo says which photos hold a dog or a cat (microseconds each, no
photo read), and a small detector draws the boxes on those few. It stores nothing about people. `PETS_ENABLED=0` turns it off (the worker and the assets
service must agree): no job is queued, nothing is read, the Pets routes answer `404` as if they did not exist and the search words stay words.

## Contents

- [What it does](#what-it-does)
- [The check, and what the numbers said](#the-check-and-what-the-numbers-said)
- [In search](#in-search)
- [In the API](#in-the-api)
- [In the app](#in-the-app)
- [Named pets](#named-pets)
- [Privacy](#privacy)
- [Models, licences and provenance](#models-licences-and-provenance)
- [Bake-off](#bake-off)
- [Telling individuals apart](#telling-individuals-apart)
- [Settings](#settings)
- [Cost: time and memory](#cost-time-and-memory)
- [Operating it](#operating-it)
- [Tests](#tests)

## What it does

1. After a photo's `embed` job (search indexing) stores its embedding, a `pets` job is queued for it at backfill priority. On an existing library the
   worker's sweep (every 15 minutes) queues one for every photo that has a preview and a search embedding and has not been looked at: that is the
   backfill. The sweep also covers a model switch and lost jobs. A photo that is edited (rotate, crop) forgets its pets and is looked at again after its
   new embedding is made (the job waits for the new embedding rather than read the old one).
2. The job runs in a loop of its own, after everything else: it takes a job only when no thumbnail, metadata, embed, text, faces or other foreground job
   is runnable, and it shares the embed loop's one-at-a-time permit, so the models of all of them never work at the same moment.
3. **The check.** It reads the photo's stored SigLIP 2 embedding (a few kilobytes from the database) and compares it with the embeddings of 20 short
   prompts ("a photo of a dog", "a photo of a horse", "a painting of an animal", ...). The *share* of the model's attention that the dog prompts get, and the
   cat prompts get, says which kinds are in the photo; 0.25 or more confirms a kind. For most photos (no dog, no cat) that is the whole job: nothing else
   is read, no model is loaded, the photo is recorded as scanned with no pets. Details and numbers below.
4. **The boxes.** Only for a photo with a confirmed kind, the job downloads the photo's **preview** (`preview.webp`, at most 1440 px, never the original, so
   no large decode and nothing to refuse for memory), decodes it and runs D-FINE-N, a 15 MB object detector. It keeps the boxes of the confirmed kinds
   only, the strongest one if it reaches 0.10 and further ones only if they are surer (0.25) and not the same animal again. While it runs it holds 96 MB
   of the worker's byte budget (`WORKER_MEMORY_BUDGET_MB`). A photo the check confirmed and the detector found no box in is recorded as having no pets
   (3 of 129 on the evaluation set); the detector is the proof.
5. The result replaces the photo's rows in one transaction (`pet_detections`, one row per animal) and records that the photo was scanned, also when it
   has no pets (`pet_scans`, with what the check said), so running the job again never duplicates an animal and "scanned, no pets" is different from
   "not scanned yet".

What is stored per animal (migration 0036): its kind (`dog` or `cat`, a plain word: another animal is a line in code, not a new table or column), the box
(left, top, width and height as fractions of the preview, so it holds at any size), the detector's score, the model and its version, and the photo and
owner. Per photo and model: the number of animals and the two shares.

### Is the preview enough?

Mostly. A preview is 1440 px on its long edge and the detector looks at 640 x 640 of it, so an animal needs to be about 3 % of the photo's long edge to be
found as a box. The check works on the whole photo at 224 px and is better at small animals than that sounds (below). The measured weak spot is a small dog far away
in a big scene: 4 of the 21 "small" evaluation photos were missed (and a pet that is only a few pixels is not a pet in the photo for anyone).

## The check, and what the numbers said

The ticket asked to measure the candidates before choosing: SigLIP 2 zero-shot on the embeddings the library already has, an object detector, or both.
Everything below is on the evaluation set described under "Bake-off" (216 photos, 81 with a dog, 58 with a cat, 128 with either, 88 with neither,
including statues, paintings, toys, other animals and wild cats), at each candidate's best threshold for "has a pet" unless a row says otherwise.

| Candidate | F1 | precision / recall | F1 with the threshold chosen on one half, scored on the other | time per photo (2 threads) | size |
| --- | --- | --- | --- | --- | --- |
| NanoDet-Plus-m-1.5x alone | 0.874 | 88.1 % / 86.7 % | 0.855 | 29 ms | 3.6 MB |
| YOLOX-s alone | 0.892 | 85.1 % / 93.8 % | 0.880 | 232 ms | 34 MB |
| D-FINE-N alone | 0.894 | 86.8 % / 92.2 % | 0.885 | 134 ms | 14.6 MB |
| RT-DETR-R18vd alone | 0.930 | 92.3 % / 93.8 % | 0.917 | 583 ms | 79 MB |
| SigLIP 2, probability of the best dog or cat prompt | 0.913 | 88.3 % / 94.5 % | 0.911 | 0.02 ms | none (the library's embedding) |
| **SigLIP 2, share among the 20 prompts (the check)** | **0.965** | **96.1 % / 96.9 %** | **0.961** | **0.06 ms** | **none** |
| the check, then D-FINE-N boxes (**as shipped**) | **0.953** | **96.0 % / 94.5 %** | 0.953 | 0.06 ms for most photos; 129 ms more for a photo with a pet | 14.6 MB |

For "contains a dog" and "contains a cat" at the shipped thresholds: dog 97.3 % precision and 87.7 % recall (81 photos), cat 91.5 % and 93.1 % (58 photos) for the pipeline
as shipped; the check alone has dog 97.3 % / 90.1 % and cat 90.2 % / 94.8 %. The evaluation photos' own embedding was computed in the bake-off from the full photo
scaled to the 1440 px preview (the model reads 224 px of it), as the worker's embed job would; the library's stored vector is half precision, which the check
was not measured on separately (the cosines differ in the fourth decimal).

What the table says:

- **The check beats every detector for "is there a dog or a cat in this photo?"**, and costs nothing: the embedding exists already. A detector alone is
  fooled by statues, paintings and wild cats (it sees a cat-shaped object) and misses small and half-hidden animals that the whole-photo embedding still
  recognises. Of the detectors only RT-DETR comes close (0.93), at 79 MB and half a second a photo. The "pet is a dog or a cat" question is not
  what a detector is for; its boxes are.
- **Combining them** (the check decides which kinds are in the photo, the detector says where) is 0.012 below the check alone, because 3 photos the check confirmed
  got no box and are dropped; and it is the only one of the two that gives **boxes**, which the later per-pet pages need. The detector runs only for the photos the check
  confirmed, so its cost is paid for 5 to 20 photos in 100 of a family library (60 in 100 here, half of the evaluation set being pets).
- **The detectors' own thresholds** are chosen on the same photos they are scored on (the "one half / other half" column is the honest figure), and D-FINE-N's probabilities
  are small: its best threshold alone is 0.21, where the shipped box floor of 0.10 is only a floor, because the check has already said an animal of the kind is in the photo.
  D-FINE-N was chosen over RT-DETR for the boxes: 5 times smaller, 4 times faster, 0.04 F1 behind when each decides alone (0.894 against 0.930), and as the second step behind the
  check 0.012 behind (0.953 against 0.965: about 2 photos in 216).
- The OpenCV Zoo's block-quantised 8-bit variants of YOLOX and NanoDet do not load in ONNX Runtime (a `Resize` without sizes or scales, a `DequantizeLinear` with a block
  size), so they are not candidates.

**How the prompts were chosen, and what that does to the numbers (read this).** The check is a softmax over prompts: a photo's dog share is how much of the attention
the dog prompts get *among the others*, so a horse, a statue of a dog or a plush toy gives the dog prompts a low share because another prompt fits better. The 16 other
prompts are the things a dog or cat photo is told apart from: other animals (horse, sheep, fox, wolf, rabbit, bird, cow, wild animal), a toy, a statue, a painting of an
animal, and what photos are otherwise of (a person, a landscape, a street, a room, food). **They were written by the person who also chose the evaluation set's hard
negatives**, with the same list of look-alikes in mind, so the check is *tuned to this set's kinds of negative*. The ablations show how much that is worth:

| Prompts | F1 | false alarms at the best threshold (animal / art / wild) |
| --- | --- | --- |
| the 20 prompts of the check (shipped) | 0.965 | 2 / 1 / 2 |
| without the toy, statue and painting prompts | 0.942 | 2 / 6 / 1 |
| only dog and cat and five generic ones (person, landscape, street, room, food) | 0.893 | 9 / 7 / 6 |

With generic negatives alone the check is as good as the best detector alone (0.893 against 0.894), which is the honest floor. The evaluation set is also
harder than a family library on exactly these points: half of its photos hold a pet, and a quarter are other animals, statues, paintings, toys and wild cats, which are
rare in a family library; the photos of people, landscapes, streets, food and rooms that fill a family library were never false alarms in any variant. So **expect
the precision in a real library to be at least what is measured, and the recall to be what is measured (about 95 %)**, but the prompts have not been tried on a
library the way they were tried here, and this is the first thing to look at if a library disagrees.

**The threshold** (0.25) is the best F1 on this set (0.264 exactly) rounded to a plateau: F1 is 0.947 at 0.20, 0.965 at 0.25, 0.957 at 0.30, 0.948 at 0.35, so anywhere from
0.20 to 0.35 is within 2 points and the figure was not tuned to a photo. Lower finds more small and partial animals (recall 98.4 % at 0.20) and lets in more statues and wild cats
(precision 91.3 %); higher is the other way.

**Limits of the set**, stated plainly: it is small (216 photos, 128 with a pet); self-assembled from Wikimedia Commons uploads that are public domain or CC0, so mostly
photos people chose to upload (clear, well framed, many close-ups) with some museum art and old photographs, and not phone snapshots of a family's pet in a busy room; labelled by one
person by eye; and it cannot say how the check treats breeds, coat colours, puppies and kittens, dogs in costumes, or animals in unusual poses, except that those it holds were found.
The thresholds, the prompts and the "small" and "partial" subsets (21 and 10 photos) rest on very few photos. The check cannot tell a dog from a statue of one
when the statue is photographed like a dog (the one art photo it still takes), and no model here can tell a pet from a stray or a wild dog.

### Why the check needs SigLIP 2

The prompts' embeddings are SigLIP 2's, and the share's threshold is its. With `SEARCH_MODEL=clip-b32` (the model SigLIP 2 replaced) there is nothing to read the check with and
**Pets is off** (the worker and the assets service say so in their logs; no job is queued, the routes answer `404`). The prompt vectors are computed once with the text tower and kept in
`crates/cyanotype-clip/data/pets-prompts-siglip2-b16.f32` (61 KB, 20 vectors of 768 numbers), so neither the worker nor the check needs the text model (283 MB);
`cargo run --release -p cyanotype-clip --example pet_prompts` writes the file and a test fails when it no longer matches the model.

## In search

- **Typed words.** "dog", "dogs", "puppy", "puppies", "doggo", "pup", "cat", "cats", "kitten", "kitty", "pet" and "pets" in a query become the `pets` filter and a removable
  chip (**Dogs**, **Cats** or **Pets**; a dog word and a cat word together are any pet, "pet dog" is a dog). What is left of the words is what the model searches for, among
  those photos: "dog on the beach" is the beach photos with a dog, "dogs" alone lists them newest first. The words that introduce a pet go with it ("with a dog", "my cats",
  "a dog or a cat"). A word that is part of something else is left alone: "hot dog", "corn dog", "prairie dog", "big cat", "wild cat", "copy cat", "dog food", "dog toy",
  "dog statue", "cat tree", "cat litter", "cat walk", "pet shop", "dog years" and the like stay words for the semantic search. A person's name read first keeps its words
  ("with Cat" is a person called Cat if the owner named one; "cat beach" is a cat). The x on the chip searches again with `skip=pets`, which gives the words back to the
  semantic search over every photo. An explicit `pets` filter wins over the words. While Pets is off nothing is read.
- **The filter.** `pets=any|dog|cat` on `GET /assets/search` (and on `similar`, and in a `search` bulk scope): photos with a dog, a cat or either found in them, combined with
  text, dates, places, people, tags, ratings, type, album and the rest. Hidden photos are in no search, whatever the filters say; trashed ones and other owners' photos neither.
- **The facet.** `GET /assets/search/facets` has `pets: [{value, count}]` for `any`, `dog` and `cat` (0 entries left out; absent while Pets is off), counting visible live photos like the
  other facets.

## In the API

All of it is the owner's own session or API key (read scope for reads, write scope for the bulk actions that take the scope) and nothing else. While Pets is off every route
below but the status is a `404`. A `kind` that is not `any`, `dog` or `cat` is a `400`; ids of someone else's photos are `404`. A photo is in the Pets view when it is live (not
trashed, not a RAW half, archived ones too) and not hidden unless the session has the Hidden view unlocked; `GET /assets/search` never lists a hidden photo, as for every filter.

| | |
| --- | --- |
| `GET /assets/pets/status` | `{enabled}`: the only Pets route that answers while the feature is off (a client reads it to show or hide Pets). |
| `GET /assets/pets` | `{any, kinds: [{kind, photos, cover}], scanned, total}`: how many photos this session may see have a pet of any kind, and per kind (`dog`, `cat`: every kind is listed, with `photos: 0` and `cover: null` when there is none). `cover` is `{asset_id, box: [x, y, w, h], preview_url}` of the clearest animal of that kind (score times area; `box` is fractions of the preview image the URL serves, so a client crops it with CSS). `scanned` and `total` are how far the scan has got for the owner's photos that can be scanned (a preview and a search embedding). |
| `GET /assets/pets/photos?kind=any\|dog\|cat&limit&cursor` (or `before`) | The photos with a pet of that kind (default `any`), newest first, with the items, ordering and keyset paging of `GET /assets` (`{items, next_cursor, next_before}`). |
| `GET /assets/pets/timeline?kind=` | `{total, days: [{day, count}]}`: the same photos counted per day as `GET /assets/timeline` does, so the view can be laid out and scrubbed like the library. |
| `GET /assets/{id}/pets` | `{items: [{id, kind, box: [x, y, w, h], score, pet}], scanned}` (`pet` is `{id, name}` or null, see Named pets): the animals found in one photo, strongest first; `scanned` is false while the photo has not been looked at (`items` is empty either way). For the viewer's boxes and, later, a pet's crop. |
| `GET /assets/search?pets=any\|dog\|cat` | The filter above. `parsed.chips[]` has `{kind: "pets", label: "Dogs" \| "Cats" \| "Pets", matched, pets: "dog" \| "cat" \| "any"}` when words became it. `skip=pets` turns the reading of words off. |
| `GET /assets/search/facets` | `pets: [{value: "any" \| "dog" \| "cat", count}]` while Pets is on. |
| bulk requests: `{"scope": {"pets": "any\|dog\|cat"}, "except": [...]}` | Like `{"person": id}` and `{"album": id}`: the photos of the Pets view for that kind, resolved inside the request's transaction (favorite, rating, tags, hide, archive, trash, restore, edits, albums, shares, zip). A `search` scope with a `pets` filter is **refused (404) while Pets is off**, never read as "no filter": a bulk trash must not widen to the whole library because a switch was flipped. |
| smart albums: `{"field": "pets", "kind": "any\|dog\|cat"}` | "A dog, a cat or either was found in the photo" (the rule editor's *Pets* rule). Saving one while Pets is off is a `404` and a saved rule matches no photo while it is off, in the album, its counts, its bulk scope and its share link (never "no rule", which would be every photo). The album keeps only the kind. |
| `GET /assets/processing` | has a `pets` entry while the feature is on: `{scanned, total, found, unfingerprinted, queued, running, failed, model, percent}`; a failed `pets` job is also in the failure list and "Retry failed" requeues it. |

Pick names for a UI from these: the Pets entry of the library menu is shown when `status.enabled`; the page is `/assets/pets` for the header and the kind tabs (the counts and
covers), `/assets/pets/photos` and `/assets/pets/timeline` for the grid (the library's own timeline component), bulk actions through `{"pets": kind}`, "Search with filters"
through `pets=`; the viewer's boxes come from `/assets/{id}/pets`.

## In the app

Everything below is shown only while Pets is on (`GET /assets/pets/status`, read once when the app opens; a failed answer counts as off) and the entry points only once at least one photo has
a pet. While it is off `/pets` is a "Page not found", the sidebar has no Pets, Collections has no Pets row, and `pets` in a search address, the Pets filter and the Pets rule do not exist.

- **Pets** (`/pets`, `?kind=dog|cat`). A page of its own: the photos with a dog or a cat as the library's timeline (months and days, the scrubber, selection, the viewer, the slideshow). Under
  the title a switch **All / Dogs / Cats** (a kind with no photos is not offered, and with one kind there is no switch). The menu has **Save as album** and **Search with filters** (`/search?pets=`).
  Selecting works like a person's page: "Select all" selects the whole view by the scope `{"pets": kind}` (favorite, rate, tag, archive, hide, add to or save as an album, share, download as a
  zip, move to trash), not by the photos the browser happened to load. The page reads the counts again every 15 seconds and the timeline follows them. Boxes are not drawn in the viewer yet.
- **While the library is being looked through** the page says how far the worker has got ("Looked at 1,200 of 5,000 photos", a progress bar): as the only content while nothing is found yet,
  above the photos once some are. When the whole library has been looked at and nothing was found, it says so.
- **Collections** has a **Pets** row: one round card per kind that has photos ("Dogs, 6 photos"), cropped by CSS from the cover's box like a person's face (nothing is made on the server),
  with a link to the page. The sidebar's Library list has **Pets** next to People.
- **Search.** The words "dogs", "cat on the sofa" show the parsed chip (**Dogs**, **Cats**, **Pets**); its x searches again with `skip=pets`. The Filters sheet has **Pets**: Any, Dogs or cats,
  Dogs, Cats, with the facet's counts. "Select all" and "Save as album" in the results use the `pets` filter in the `search` scope, also when it came from the words.
- **Smart albums.** The rule editor offers a **Pets** rule (Dogs or cats, Dogs, Cats) and words it "Pets: dogs".
- **Progress.** The Pets page shows the scan (above). `GET /assets/processing` also reports `pets`; the library's processing banner does not draw it (like search indexing and text it is reported apart
  from the work in progress), but a failed `pets` job is in its list of failures as "Finding pets", and "Retry failed" requeues it.

## Named pets

The owner makes a pet ("Biscuit", a dog), tags a few photos of it (five to ten is plenty), and the app **proposes** more: "Is this Biscuit?", best first, in two tiers
(*likely* and *maybe*). The owner confirms or refuses in bulk, and **every answer counts**: a confirmed animal moves the centre the next proposals are measured from, a refused
one is never proposed for that pet again. A pet has a page of photos (a timeline like a person's), a cover, a name that can be changed, and can be hidden, merged and deleted.
The app's screens for it are described under [Named pets in the app](#named-pets-in-the-app), after the routes they use; everything is under `PETS_ENABLED` (off: every route a `404`, and none of the screens is there) and is the signed-in owner's alone.

### How it works

1. **A fingerprint per animal.** The `pets` job crops each animal's box out of the preview (a square around the box, 10 % bigger, `cyanotype_pets::crop`), embeds the crop with **DINOv2-small**
   (8-bit, the class token, 384 numbers) and stores it with the animal (`pet_detections.embedding`, with `embed_model`, which names the model *and* the crop recipe: a different one is
   fingerprinted again, and numbers of two models are never compared). A crop whose shorter side is under 64 px of the preview gets none (retrieval at 64 px is 0.89 mean average precision,
   0.93 at full size; 48 px 0.85; 32 px 0.76), and is recorded as tried. It is made in the job that finds the animal, from the same decoded preview.
2. **Animals found before named pets** have no fingerprint: the worker's sweep queues their photos, and the job then **only fingerprints them, without finding them again** (same rows, same
   ids), so a library is backfilled without a rescan and without disturbing anything. `GET /assets/processing` has `pets.unfingerprinted` while it goes on.
3. **Proposals.** An animal is scored by its cosine similarity to the **centre** (the mean) of the fingerprints of the animals in the pet. Only free animals of the pet's kind count: not another
   pet's, not one refused for this pet, not one in the trash, the Hidden view (unless unlocked), another owner's, or in a photo being scanned again. A score of at least **0.72 is *likely***,
   **0.55 *maybe***; below that nothing is proposed (`crates/cyanotype-core/src/named_pets.rs`; the numbers are from the table in "Telling individuals apart"). At most 300 are listed.
   The count each pet shows in the list (`pending`) is the same number, for all pets in **one** query.
4. **Decisions are the owner's and survive a rescan.** `pet_detections.pet_id` is set only by the owner (there is no automatic assignment), `pet_rejections` holds "not this pet". A new scan of
   a photo (a rotate or crop, a new detector or fingerprint model) replaces its animals with new rows, and `pets::store` hands what was decided to the animal that takes each one's place: matched
   one to one within a kind, first by fingerprint (cosine at least 0.80, same model only: the same animal re-found), then by overlapping boxes (IoU at least 0.5), then, if the photo had
   one animal of the kind and the new scan has one, those two (a Rotate moves the box and changes the crop: DINOv2 says only 0.65 for the same animal turned, against 0.34 median for
   look-alikes, so a fingerprint cannot be trusted across a rotate; with several animals in the photo nothing is guessed). The pet it was in, the pets it was refused for and being a pet's cover
   are carried. After a rotate or crop the animals the owner decided about wait, unread, for the new scan; the others are forgotten (`forget_and_requeue`).

### The routes

All under `/assets/pets/named` (`/assets/pets` itself is the summary of the kinds). Reads need a read key, changes a write key. Ids of someone else's pets, animals and photos are `404`, the
same as unknown ones.

| | |
| --- | --- |
| `GET /assets/pets/named`, `GET /assets/pets/named/{id}` | `{items: [{id, name, kind, hidden, photos, detections, pending, cover}]}`, most photos first; `kind` is `dog` or `cat`; `cover` is `{detection_id, asset_id, box: [x, y, w, h], preview_url}` or null (the box is fractions of the preview, cropped with CSS like a person's face; the chosen cover, else the clearest animal). Counts are of the photos this session may see (live, not a RAW half, not hidden unless the Hidden view is unlocked); a pet only hidden photos have is not listed to a locked session (its page is a `404`). `pending` is how many proposals wait (0 for a hidden pet), for all pets in one query. |
| `POST /assets/pets/named {name, kind, asset_ids?, detection_ids?}` | Make a pet from photos. `name` is trimmed, 1 to 100 characters, no control characters, **two pets may share a name**; `kind` is `dog` or `cat`; at most 1,000 ids and 500 pets per owner. Answers `{pet, assigned, ambiguous, skipped}`: for each photo in `asset_ids` its *free* animals of the kind decide: **one** joins the pet; **several** and none joins: the photo is in `ambiguous` as `{asset_id, preview_url, candidates: [{detection_id, box}]}` for the caller to choose from (then `POST .../photos {detection_ids}`); **none** or not looked at yet: in `skipped` as `{asset_id, reason}`, `reason` being `no_animal`, `not_scanned`, `already` (the photo already has an animal of this pet) or `in_other_pet`. `detection_ids` name animals directly (the caller's explicit word: one that is another pet's moves, and is refused for the first; of the wrong kind is a `400`). A photo or animal that is not the owner's, or not visible to this session, is a `404` and **nothing** is made. The pet is made even when no photo could be placed. |
| `PATCH /assets/pets/named/{id} {name?, hidden?, cover_detection_id?}` | Rename (a blank name is a `400`: a pet is always named), hide or show, choose the cover (must be one of the pet's animals in a photo this session may see, else `400`). `DELETE` frees the animals (they stay, with their fingerprints) and the photos are untouched; the refusals for it go with it. Audited as a count only. |
| `POST /assets/pets/named/{id}/merge {into}` | The first pet's animals go to the other (same kind only, else `400`), which keeps its name; "not the first" becomes "not the second". Audited as a count only. |
| `GET /assets/pets/named/{id}/detections?limit&cursor` | The animals a cover can be chosen from (`{detection_id, asset_id, box, preview_url}`, clearest first, `next_cursor` while more). |
| `GET /assets/pets/named/{id}/photos?limit&cursor\|before`, `GET .../timeline` | The pet's photos, same items, ordering and keyset paging as `GET /assets`; `{total, days}` for the scrubber. Live photos, archived ones too, hidden ones only with the Hidden view unlocked. |
| `POST /assets/pets/named/{id}/photos {asset_ids?, detection_ids?}` | Add photos or animals: the same answer and rules as making a pet (`{pet, assigned, ambiguous, skipped}`). |
| `POST /assets/pets/named/{id}/photos/remove {asset_ids?, detection_ids?}` | **"Not this pet"**: the pet's animals in those photos (or those animals) are freed **and refused for the pet**, never proposed for it again; a removed cover is cleared. `{removed}`. Adding the animal again by hand forgets the refusal. |
| `GET /assets/pets/named/{id}/suggestions?limit&tier=likely\|maybe` | `{items: [{detection_id, asset_id, box, preview_url, similarity, tier}], counts: {likely, maybe}}`, best first (`limit` default 24, at most 100; the counts are of everything proposed, not of the page). A pet with no fingerprinted animal yet has none. |
| `POST /assets/pets/named/{id}/suggestions/answer {confirm?: [detection_id], reject?: [detection_id]}` | Answer many at once (at most 1,000): `{confirmed, rejected, skipped}`. Only free animals of the pet's kind change; one already another pet's or this one's (to confirm), or already refused (to reject), is `skipped`, and an id that is not the owner's, not visible, or of another kind is a `404` with **nothing** changed. |
| `POST /assets/pets/detections/{id}/assign {pet_id}`, `POST /assets/pets/detections/{id}/reject {pet_id}` | The same for one animal (`{pet}` / `204`); moving an animal from another pet refuses it for that one; refusing an animal that is in the pet is a `400` (remove it instead). |
| `GET /assets/{id}/pets` | Its items gain `pet: {id, name}` for an animal the owner put in a (not hidden) pet, else `null`. |
| `GET /assets/search?pet=<id>,<id>` (also `similar`, and the `search` scope) | Photos where **all** the named pets are, together (at most 10 ids; `400` for a malformed one or more than 10; an id that is not the owner's matches nothing, exactly like one that never existed). Combines with every other filter, `pets=`, people and text. Ignored while Pets is off. |
| the words of a query | The owner's **named, not hidden** pets with a photo a search can show are read out of the words like people's names (after dates, places and people; before "dog", "cat" and "pet"): at the start or end of the query or after *with, of, and, featuring, plus, including*; the longest name wins; a name two pets share, a name a person has (the person is read first) and names under three letters are never matched; single common words need an introducing word. A chip `{kind: "pet", label, matched, pet: [{id, name}]}`; `skip=pet` gives the words back; an explicit `pet` filter wins. |
| bulk requests: `{"scope": {"pet": "<id>"}, "except": [...]}` | The pet's photos as its page lists them, resolved inside the request's transaction (so **Save as album** is `POST /assets/albums {title}` then `POST /assets/albums/{id}/assets {scope: {pet}}`). Pets off or an id that is not the owner's: `404`; a `search` scope with a `pet` filter is refused while Pets is off, never read as "no filter". |
| smart albums: `{"field": "pet", "ids": [...]}` | "All of these pets are in the photo" (1 to 10 of the owner's own, checked when it is saved: another owner's id is a `404`). The album keeps only ids; a deleted pet leaves a rule that matches **nothing**, and a merge is not followed (edit the rule). Pets off: saving is a `404` and a saved rule matches nothing. |

**The title of "Save as album" starts empty**, as for people: an album's title is shown to everyone it is shared with and is written into the recovery manifest, so a pet's name must not be put there by default.

### Named pets in the app

Shown only while Pets is on, like the rest of "In the app" above, and built from People's own parts (the round cover crop, the "Is this X?" question, the timeline, the selection bar, the rename and merge sheets).

- **Naming a pet.** *Select* photos in the Photos, Search or an album's grid (or on the Pets page: **Name a pet**, in the page or in its menu, turns selection on and asks for the photos of the pet) and choose **Name pet…**
  (the "⋯" of the selection bar on a phone). The sheet asks for a name and a kind (dog or cat), or offers the pets there already (**Add photos**). It needs the photos one by one, so it is off while the whole view is
  selected. What the server answered is told in plain words: a photo with **several animals of the kind** is shown with each outlined and numbered, and the owner taps the right one (*Skip this photo* leaves it
  out; the choices are sent as `detection_ids`); photos that could not join are listed with the reason ("1 photo has no dog that was found", "…has not been looked through for pets yet", "…is already in Name",
  "…belongs to another pet already"). The toast says what was made and opens the pet.
- **The Pets page** has **Your pets** above the photos: a round cover, the name and the number of photos for each pet, hidden ones last and dimmed, and a count in the corner when "Is this X?" proposals wait. With no pet
  yet it says how to make one. Collections' Pets row starts with the named pets (not the hidden ones), then the kinds.
- **A pet's page** (`/pets/{id}`): the cover, the name, the kind, the photos and what waits to be confirmed; the photos as the library's timeline (selection, "Select all" by the `{"pet": id}` scope, the viewer, the slideshow,
  every bulk action). The "⋯" menu has **Rename** (a pet is never without a name), **Change cover** (the pet's animals as crops, clearest first), **Hide**/**Show**, **Save as album**, **Search with filters**
  (`/search?pet=`), **Merge into…** (only pets of the same kind are offered) and **Delete pet** (the photos stay). Selecting photos offers **Use as cover** (one photo) and **Not this pet** (asks; the animals are freed and refused
  for the pet, which never proposes them again; off for a whole-view selection). The page reads the counts again every 15 seconds.
- **"Is this {name}?"** on the pet's page, under the header: the *likely* animals first, a switch to the *maybe* ones when there are both, each as a crop with **Yes** and **No**, tap the crop for the whole photo with the
  animal outlined. **Yes to these N** / **No to these N** answer everything shown at once (after asking: the second is for good). An answer takes the tile away at once and puts it back with a message if the server
  refuses; a Yes reads the proposals again, because it moves the pet's centre. There is no undo: the server has no way back to "not answered" (a Yes can be taken out with *Not this pet*, which refuses the animal).
- **The viewer.** The Info panel has a **Pets** row while Pets is on and the photo has animals: each with a crop and the pet's name (a link to its page) and **Not {name}** (takes it out of the pet and refuses it for it),
  or "not named" and **Add to pet…** (the same sheet, for one animal: a new pet, or one of its kind). One switch, **Show pets on the photo** (**Show faces and pets on the photo** when the photo also has faces; it replaces the faces' own switch and sits in People, else in Pets), outlines every animal with a label (the pet's name, else "Dog" or "Cat"); a tap on a box opens the pet's page, or the same sheet for an animal nobody named. It stays as set from photo to photo, and the share viewer never loads pets.
- **Search.** A pet's name in the words becomes a chip with the name (its x gives the words back with `skip=pet`); `pet=<id>,<id>` in the address is a chip too ("Name and Name"); the filter sheet has **Named pets**
  (not the hidden ones), and "Save as album" and "Select all" of the results use the filter. **Smart albums** have a **Pet is** rule (one or more named pets, all of them in the photo).

### What is not there

- No automatic assignment: no job ever puts an animal in a pet. A new pet has proposals only once its photos have fingerprints (seconds after the job; a backfilled library takes the sweep's time).
- No "not a pet" mark for a detection that is no animal (a plush toy found as a cat): refuse it for the pet, which also keeps it out of that pet's proposals.
- A refusal is an exclusion, not a negative example: near-duplicates of a refused animal (a burst) are proposed separately and have to be refused too (the owner can do it in bulk). Not measured.
- No pet-level merge of decisions across kinds (a dog is never a cat's) and no cross-owner anything.

## Privacy

- **It finds animals, not people.** `pet_detections` and `pet_scans` hold a kind, a box, a score and two shares per photo, and (for named pets, below) a fingerprint of the animal's crop: no face and no name. A box that happens to
  contain a person's face in the middle of a dog's picture is a box.
- **Deleted with the photo.** A permanent delete (`apps/assets/src/purge.rs`) removes the photo's rows in the same transaction as the photo; the foreign keys cascade from
  `assets` and from `users` too, so deleting an account removes its detections.
- **Owner-only, never shared.** Share links, the public pages, downloads, the recovery manifests and the CLI import never include detections or boxes (a test shares photos
  with dogs, and a smart album whose rule is "a dog", and searches every public response and the owner's share responses for the kinds, boxes, scores and ids). The routes are for
  the signed-in owner, and a photo in the Hidden view is invisible to a session that has not unlocked it.
- **Derived data, not backed up as such.** The detections are not in the recovery manifests (`docs/recovery.md`): they are found again by a rescan, which the worker queues by itself once
  the photos have their embeddings again. A smart album's `pets` rule is only a kind and travels with the album's rules.
- **Named pets.** A pet's name is personal data: it is stored in `pets.name` (migration 0037), shown only in the owner's session, and **never written to the bucket, the audit trail (a count is) or a
  log**. A fingerprint is a picture-derived vector of an animal's crop and holds no face of its own, but a crop can contain whatever is in the box. **Never on a public page:** a test makes a pet, a
  smart album that follows it and an album from it by scope, shares them, and searches every public response, the owner's album and share lists and the recovery manifest for the name, the ids and the
  boxes. Deleting the account or the photo deletes its animals, rejections and (with the account) pets.
- **Recovery: the same as People, deliberately.** Pet names, which animals are in which pet and "not this pet" are **not in the recovery manifests** (`docs/recovery.md`): after a bucket-only rebuild
  the animals are found and fingerprinted again, and the owner makes the pets again, which is five photos each (People's names are in the same position, `docs/faces.md`); the Postgres dumps do hold
  them. A smart album with a `pet` rule keeps its title and everything else in the manifest, **without its rules** (their ids would mean nothing, and are private), exactly like a people rule
  (`manifest::has_private_rule`). Putting names in the manifests would have meant a new line type, a restore that matches animals by something stable (a photo's hash and a box) and a change of the
  manifest's privacy statement, for data that is a few minutes to make again; if that changes, the line type is the place.
- **One command removes everything.** `cyanotype pets delete [--email ADDRESS] --apply` deletes every detection, every scan record, every named pet (names included), every "not this pet" decision and every queued or finished `pets` job, for one
  account or for all; without `--apply` it only counts. Switch Pets off and restart the worker first, or the sweep finds the photos again. Run it where `migrate` runs
  (`docs/deploy.md`: `dc run --rm reindex pets delete --apply`).

## Models, licences and provenance

The rule (as for faces and text): only models whose weights may be redistributed inside our Docker images. The licence texts were read from the model repositories' own files at the
pinned commits; the repository itself is AGPL-3.0, so AGPL weights would have been allowed but none was needed.

| Model | Used for | Licence | Source (primary) |
| --- | --- | --- | --- |
| **D-FINE-N** (`dfine-n.onnx`, 15 MB, COCO only, 4 M parameters, 42.8 AP) | the boxes | **Apache 2.0** (Yansong Peng et al., `Peterande/D-FINE`) | the ONNX export that Hugging Face's transformers.js team published in `onnx-community/dfine_n_coco-ONNX` at commit `380d2839c327efaf65dd0fe0c2c10ab7fadd5473` (2025-04-30), `onnx/model.onnx`, from `ustc-community/dfine_n_coco` (the Hugging Face port, whose model card states Apache 2.0). Neither Hugging Face repository has a licence file, so the licence text shipped next to the weights is the one in `Peterande/D-FINE` at commit `956d1709314c2c6a4df6f34de232054578a7449f` (2026-08-19, `LICENSE`, Apache 2.0, which its README names for the code and the released weights) |
| **DINOv2-small** (`dinov2-small-int8.onnx`, 24 MB, 21 M parameters, the class token, 384 numbers) | the fingerprint of each animal's crop, for named pets | **Apache 2.0** (Oquab et al., Meta; the model card of `facebook/dinov2-small` and the repository's LICENSE) | the 8-bit ONNX export that Hugging Face's transformers.js team published in `onnx-community/dinov2-small`, pinned to its commit of 2024-10-08 (`onnx/model_int8.onnx`); licence text from `facebookresearch/dinov2` at `7764ea0` |
| **SigLIP 2 base** (the search model, 224 px) | the check: its image embedding is read against prompts embedded with its text tower | **Apache 2.0** (Google) | already in the worker and assets images for search (`models/siglip2.lock`); the prompt vectors are in the repository |

Both are pinned by exact URL and sha256 in `models/pets.lock` (and `models/siglip2.lock`), fetched by `scripts/fetch-models.sh`, verified, and installed with the licence text
(`LICENSE-dfine.txt`, `LICENSE-dinov2.txt`) in the worker image under `/opt/cyanotype/models/pets/`, as Apache 2.0 requires. All are older than 14 days. DINOv2 was trained on LVD-142M (images curated from web crawls); the
caveat below applies to it as well.

**A caveat the licence files cannot settle.** The licences cover the weights and files in those repositories. D-FINE was trained on COCO (images from Flickr under several licences,
annotations CC BY 4.0) with an ImageNet-pretrained backbone, DINOv2 on a curated web-image set, and SigLIP 2 on web data; those datasets have their own terms, which we cannot verify or grant. D-FINE's weights are
distributed by its authors and Hugging Face under Apache 2.0; if that residual risk matters for your use, read the datasets' terms.

**Considered and not shipped** (all measured above except where it says why not):

| Candidate | Licence | Why not |
| --- | --- | --- |
| RT-DETR-R18vd (`PekingU/rtdetr_r18vd`, ONNX export in `onnx-community/rtdetr_r18vd`) | Apache 2.0 (model card) | the best detector alone (F1 0.93) but 79 MB, 583 ms a photo and about 100 MB more resident memory than D-FINE-N (a peak of 400 MB against 296 MB in the same harness), for 0.012 F1 over D-FINE-N behind the check |
| YOLOX-s (OpenCV Zoo `object_detection_yolox`, commit `26cc381e4d2594bb9f47a26eb8fd96c94a13660d`) | Apache 2.0 | slower and larger than D-FINE-N and no better as the second step (F1 0.931 behind the check: it missed 14 pets) |
| NanoDet-Plus-m-1.5x (OpenCV Zoo `object_detection_nanodet`, same commit) | Apache 2.0 | 3.6 MB and 29 ms, F1 0.953 behind the check, but weaker boxes for the later per-pet crops (COCO AP 30.4 as the Zoo measures it, against 42.8 for D-FINE-N) |
| YOLOv8/v10/11 (Ultralytics) | AGPL-3.0 | excluded: no need for AGPL weights when Apache 2.0 ones measure as well, and the weights' terms (trained with Ultralytics' tooling) are less clear than the licence |
| the block-quantised 8-bit variants of YOLOX and NanoDet | Apache 2.0 | do not load in ONNX Runtime |

## Bake-off

### The evaluation set

No dog-and-cat set with a licence that clearly permits redistribution and this use was found that also holds the hard cases (the usual ones, COCO, Open Images and Oxford-IIIT Pets,
are Flickr photos with their photographers' licences, or carry no statement). So the set is self-assembled like the face set (`docs/faces.md`) from Wikimedia Commons files that
the Commons page itself marks **CC0** (179 files), **Public domain** by the uploader (`PD-self`, 33) or by the author (`PD-author`, 4):

- `crates/cyanotype-pets/tests/eval/pets.tsv` lists the 216 photos (label by eye, group, a note, size, sha256, the licence category and the Commons page id; no titles, which can
  hold names). The files are the **original uploads**, pinned by sha256; `scripts/fetch-pets-eval-set.sh [DEST]` looks each page id up on the Commons API, then downloads and
  verifies the files (default `models/pets-eval`, 384 MB; files over 4 MB were left out unless the animal is small or partial).
- How it was made: Commons was searched (its own search, restricted to those licence categories) for 7 groups of words (dog and cat breeds and situations; "dog and cat" together;
  people with pets; other animals; things that look like pets: statues, sculptures, paintings, toys; everyday scenes), about 440 candidates were looked at on contact sheets, and each kept
  photo was labelled **by eye** (not by any model's score): `dog`, `cat`, `both` or `none`, and photos where the answer was not clear (something that might be a dog, a name on a badge,
  a close-up of a stranger's face) were left out. The licence of each file was read from its Commons page when it was fetched, and a file dual-licensed with anything else was dropped.
- Its make-up: 128 photos with a pet, 70 with a dog only, 47 with a cat only and 11 with both (21 of them with an animal that is small in the picture, 10 with one partly hidden), and 88 with neither: 40 plain scenes, 16 statues, paintings and
  drawings of animals, 6 big cats and coyotes, 18 other animals (horse, sheep, goat, deer, rabbit, pig, cow, alpaca, donkey, guinea pigs, hamster, squirrel, raccoon, meerkat, parrot,
  duck, a stuffed otter), 6 photos of people only, 2 stuffed toys.
- Limits, stated plainly: see "The check, and what the numbers said". It is small, from uploaders who chose their photos, and its "neither" half is much harder than a family library's.

### What was measured, and how

`PETS_EVAL_DIR=models/pets-eval PETS_BAKEOFF_DIR=models/pets-bakeoff SEARCH_MODEL_DIR=$PWD/models cargo test --release -p cyanotype-pets --test bakeoff -- --ignored --nocapture`
(after `scripts/fetch-pets-eval-set.sh`, `scripts/fetch-models.sh` and `LOCK=models/pets-bakeoff.lock scripts/fetch-models.sh models/pets-bakeoff`). Each photo is decoded upright
(EXIF orientation applied) and scaled to at most 1440 px, as the worker's preview would be, then handed to each candidate: the detectors with the pre- and post-processing of
their publishers' reference code (`tests/bakeoff/candidates.rs`), the check and the pipeline through the shipped code. A detector's score for a kind is the strongest box of that
kind; "has a pet" is a dog or a cat above the threshold. Times are per photo, two threads, on a shared 4-core development machine (the numbers move 20 to 30 % with its
load). The tables print precision and recall per kind at a range of thresholds, the best F1 and where it falls, and the mistakes by group.

The set was labelled on contact sheets made by `cargo run --release -p cyanotype-pets --example sheet -- OUT.jpg 5 240 photos...` (each tile marked with its file number); what the job
would store can be looked at the same way with `--example draw` (a red box on every dog, a blue one on every cat, a grey frame on a photo the check passes over) and its time and memory
measured with `--example bench` (below).

## Telling individuals apart

The question for named pets is not "which photos have a dog" but "given a few photos the owner says are Biscuit, which other photos are Biscuit, and not the neighbour's look-alike?" It was measured
before anything was built, as the faces and the detectors were (`crates/cyanotype-pets/tests/individuals.rs`, ignored).

### The evaluation set

`crates/cyanotype-pets/tests/eval/individuals.tsv` lists **266 photos of 22 individual animals** (17 dogs, 205 photos; 5 cats, 61 photos): Wikimedia Commons originals whose page marks them **Public
domain** (212) or **CC0** (54); every other licence, CC-BY included, was rejected. Each animal has an anonymous id (`d01`..`d17`, `c01`..`c05`), a look-alike group in generic words (a breed or a colour) and the
Commons page id of each file: **no name, owner or title is in the repository**. `scripts/fetch-pets-id-eval-set.sh [DEST]` looks each id up on the Commons API, downloads and verifies the
files by sha256 (default `models/pets-id-eval`, 619 MB; it takes about half an hour because Commons rate-limits bots, and re-running skips what is there). The labels are the Commons category that names the
animal, **checked by eye on contact sheets** (coat colour and markings), and a photo that shows more than one animal of its kind, or a different individual, was dropped. Where look-alikes are:
two black Portuguese water dogs (one with white chest and paw markings, one all black; each other's impostors, the hard case), three German shepherds (black-and-tan, sable, a puppy), two black
Scottish terriers (colour photos, old black-and-white ones), two golden and chocolate retrievers, two black cats; the other animals are alone in their group.

**Limits, plainly.** It is smaller than the 25 to 40 animals aimed for (the named cats on Commons are almost all CC-BY and so excluded; there are 5, two of them with only 5 and 6 photos, all of about 600 px). Photos of one
animal come in sessions (the same shoot, outfit and background), and the seeds are drawn from the same sessions as what is searched: **that makes the numbers kinder than a real library, where an animal
changes with the years, the haircut and the season** (the old black-and-white photos are the least certain labels and the hardest). The pool a pet is searched in is 200 photos of other animals, not a
library's 10,000. Puppies grow, and that is not measured. Six photos had no animal of the kind found by the detector and are not scored. One animal's identity (d16) is a border collie in training photos with a
person in much of the frame.

### The setting that matters, and what was measured

For every animal with at least `k + 3` photos, 40 random draws of `k` seed photos (k = 3, 5 and 10); every other photo of the **same kind** in the set is scored against the seeds, and what would be proposed at
a threshold is counted: precision and recall of the proposals, how many of the look-alikes' photos (same group, another animal) are proposed, and in how many draws at least one look-alike is. The
animal in each photo is the one D-FINE-N boxes (the biggest, surest box of the kind); the crop is cut out and embedded by each candidate.

**Mean average precision of the ranking** (the threshold-free figure; the centre of the seeds as the rule), over all draws:

| Embedding of the crop | size / time (2 threads) | k = 3 | k = 5 | k = 10 |
| --- | --- | --- | --- | --- |
| SigLIP 2 image tower (the search model: already in memory, no new model), square crop 10 % bigger | 4-bit, in memory already / 307 ms | 0.806 | 0.810 | 0.856 |
| DINOv2-small, patch-token mean | 88 MB / 148 ms | 0.662 | 0.713 | 0.778 |
| DINOv2-small, class token + patch mean | 88 MB / 148 ms | 0.843 | 0.882 | 0.934 |
| DINOv2-small, class token, 4-bit | 17 MB / 149 ms | 0.879 | 0.901 | 0.965 |
| DINOv2-small, class token, float32 | 88 MB / 148 ms | 0.900 | 0.917 | 0.968 |
| **DINOv2-small, class token, 8-bit (shipped)** | **24 MB / 140 ms** | **0.900** | **0.918** | **0.967** |

DINOv2's class token is **0.09 to 0.11 better than the search model's own embedding** (which was trained to match photos with sentences, not animals with themselves) at the same crop, so the
new model is shipped: 24 MB, 140 ms an animal, about 90 MB resident with the detector (a run of 20 photos peaked at 273 MB with large originals decoded; the job holds 96 MB of the worker's budget). The 8-bit
build loses nothing against float32; the 4-bit one loses 0.02. The patch tokens are worse than the class token, and joining them hurts.

Other choices, by the same measure (8-bit, class token): the **crop** (box alone 0.895 / 0.918 / 0.954, box + 10 % 0.910 / 0.922 / 0.954, **square + 10 % 0.900 / 0.918 / 0.967**, square + 25 % 0.894 / 0.909 / 0.961):
within a point or two of each other, the square (which does not stretch a dog) was kept. The **rule** (centre of the seeds, best seed, mean of the best three): the centre and the mean of the best three
are the same, the best seed is 0.01 to 0.03 worse, so the centre, which is also one `avg` in SQL. **Small crops** (the animal shrunk before it is embedded, mean average precision at k = 3 / 5 / 10): 128 px 0.874 / 0.897 / 0.958,
96 px 0.860 / 0.892 / 0.955, **64 px 0.834 / 0.885 / 0.952**, 48 px 0.794 / 0.848 / 0.912, 32 px 0.712 / 0.795 / 0.874, full size 0.900 / 0.918 / 0.967: below 64 px nothing is fingerprinted.

### The thresholds

The shipped model and rule, at some thresholds (cosine to the centre of the seeds). *Look-alikes proposed* is the share of the look-alikes' photos that would be proposed; *draws with one* the share of draws in which
at least one look-alike would be.

| threshold | k | precision | recall | look-alikes proposed | draws with one | proposals per draw |
| --- | --- | --- | --- | --- | --- | --- |
| 0.55 (**maybe**) | 3 | 79.5 % | 82.8 % | 34 % | 68 % | 11.2 |
| | 5 | 80.4 % | 89.1 % | 36 % | 65 % | 13.1 |
| | 10 | 84.7 % | 93.6 % | 42 % | 67 % | 11.9 |
| 0.60 | 3 | 87.2 % | 72.3 % | 26 % | 51 % | 9.2 |
| | 5 | 85.8 % | 83.4 % | 26 % | 51 % | 11.7 |
| | 10 | 86.8 % | 90.7 % | 32 % | 65 % | 11.0 |
| 0.65 | 3 | 91.5 % | 60.6 % | 19 % | 37 % | 7.1 |
| | 5 | 91.0 % | 73.0 % | 17 % | 40 % | 9.5 |
| | 10 | 93.9 % | 86.1 % | 24 % | 53 % | 9.5 |
| 0.72 (**likely**) | 3 | 93.9 % | 41.6 % | 10 % | 23 % | 4.6 |
| | 5 | 95.1 % | 54.3 % | 5.5 % | 28 % | 6.5 |
| | 10 | 96.6 % | 68.9 % | 13 % | 33 % | 7.4 |
| 0.77 | 3 | 96.6 % | 26.4 % | 3.4 % | 9 % | 2.7 |
| | 5 | 98.1 % | 36.6 % | 0.6 % | 5 % | 4.1 |
| | 10 | 98.3 % | 48.5 % | 4 % | 17 % | 5.3 |

(Against the search model's embedding the same precision of 95 % needs a threshold of 0.91 and finds a third of the photos.) **Two tiers**, from this table: **likely at 0.72** (about 95 % of what is
proposed is the pet at every k, and about half of the pet's photos are found), and **maybe at 0.55** (about 80 to 85 % precise, 83 to 94 % of the photos found). The owner confirms the likely ones in
bulk, and looks at the maybe ones one by one. The threshold does not move with `k` because the centre of more photos is steadier but not differently scaled; fewer than three seeds are less reliable than the table says
(k = 3 is the smallest measured).

**How well it separates look-alikes: not reliably, and the app does not pretend it does.** At the maybe tier a third to two fifths of the look-alikes' photos are proposed, and in two draws out of three at
least one is; at the likely tier 5 to 13 % are and in a quarter to a third of the draws one is. Results per animal and per group were not tabulated, so which look-alikes are mixed up most is not known. That is why every proposal is
confirmed by the owner, and why the tiers exist; a look-alike's photo that was confirmed by mistake moves the centre toward the wrong animal, and removing it ("not this pet") is one click. In a real library there are
more strangers than here (so more false proposals at a threshold) and the owner's own photos span years (so lower recall), and no number here says how that works out.

### A new scan keeps the decisions

The same animal in a photo that is scanned again must keep its pet, but a rotate turns the picture: each photo was turned a quarter and its animal found and embedded again, `rotate` in the output. **The crop of the
turned photo against the crop it had**: median 0.649, 5th percentile 0.335 (DINOv2; SigLIP 2: 0.892 and 0.745), against **look-alikes' crops** median 0.341, 95th percentile 0.596, 99th 0.687, maximum 0.721
(SigLIP 2: 0.758, 0.867, 0.913, 0.945). A fingerprint is therefore good for "found again at the same size" (a re-scan by a new model version, a crop that leaves the animal alone) and **not for a rotate**: the carried
decision matches by fingerprint at 0.80 (above anything a stranger scored) and by overlapping boxes at 0.5, and a photo's only animal of a kind is matched to the new scan's only animal whatever the numbers say; with
several animals nothing is guessed after a rotate and their decisions are lost with them (the owner confirms again). Tests: `crates/cyanotype-core/tests/named_pets.rs`.

### Reproducing it

```text
scripts/fetch-pets-id-eval-set.sh                                     # the photos -> models/pets-id-eval
scripts/fetch-models.sh                                               # SigLIP 2, D-FINE-N and the 8-bit DINOv2 -> models/
LOCK=models/pets-embed-bakeoff.lock scripts/fetch-models.sh models/pets-embed   # the other builds, for the comparison
PETS_ID_DIR=models/pets-id-eval PETS_EMBED_DIR=models/pets-embed SEARCH_MODEL_DIR=$PWD/models \
  cargo test --release -p cyanotype-pets --test individuals -- --ignored --nocapture
```

It writes `embeddings.json` next to the photos (delete it to start over; `PETS_VARIANTS=dino-int8-cls/sq10` narrows the table, `PETS_DETAIL="dino-int8-cls/sq10:centre"` prints the threshold tables above), about 40 minutes
the first time on a shared machine. `-- --ignored fingerprint_cost` measures the memory and time of the shipped pair alone.

## Settings

| Setting | Where | Default | |
| --- | --- | --- | --- |
| `PETS_ENABLED` | worker, assets | on | `0`, `false`, `no` or `off` turn it off; anything else (also unset or empty) is on. Must be the same for both services. |
| `WORKER_PETS_THREADS` | worker | `1` | Intra-op threads of the detector (1 leaves a core of a 2 vCPU host to thumbnails and Postgres; 2 takes about 30 % off its time). |
| `PETS_MODEL_DIR` | worker | `<SEARCH_MODEL_DIR>/pets`, i.e. `/opt/cyanotype/models/pets` | Where `dfine-n.onnx` is. |

Fixed in code (`crates/cyanotype-clip/src/zeroshot.rs`, `crates/cyanotype-pets/src/lib.rs`, `apps/worker/src/pets.rs`): the 20 prompts, a share of at least 0.25 confirms a kind, the detector
input 640 x 640, a box floor of 0.10 (0.25 for a second box of a kind), overlap thresholds 0.5 (intersection over union) and 0.85 (of the smaller box), at most 6 boxes per kind, unload after
300 s idle, sweep every 900 s.

**Changing the model or the thresholds.** `crates/cyanotype-core/src/pets.rs` has `MODEL` (what finds the animals: the search model's reading and the detector, as one id) and
`VERSION`; `models/pets.lock` pins the detector. A new model or other prompts get a new `MODEL`, a change of how detections are made (a different threshold, a fixed bug, a **new
search model version**, since the shares and their threshold are its) a higher `VERSION`; the `pets` job's version is `VERSION`, so a new one must be higher than every version before it.
Either scans the library again, at low priority (the check alone, for most photos: minutes); the previous rows stay until a photo has the new ones, and a photo's new scan replaces them.

## Cost: time and memory

Measured with `cargo run --release -p cyanotype-pets --example bench` on the shared development machine, previews of 1440 px, one thread (the default), twenty evaluation photos:

| | |
| --- | --- |
| the check (20 dot products of 768 numbers, a softmax) | 0.06 ms |
| reading the embedding from the database | a few ms (one row of 1.5 KB) |
| detector, one photo, 1 thread | median 172 to 182 ms, p95 about 210 to 330 ms; 2 threads: median 127 ms |
| loading the detector | about 350 ms |
| the job for a photo without a pet | the check and two database writes: a few ms |

**Resident memory** (`/proc/self/status`):

| | resident |
| --- | --- |
| SigLIP 2 image tower loaded (the search job) | 127 MB |
| + detector loaded | 147 MB (+20 MB) |
| + after detecting on photos (activations, allocator slack) | 196 MB (+69 MB over the image tower alone; one detection adds at most 31 MB while it runs) |

*Does it fit the 1280m worker?* Yes. The compose file's 1280m is the 640 MB decode budget plus the models that stay resident plus runtime slack (search 140, text about 100, faces about 75 per
`docs/faces.md`); the detector adds about 70 MB when it is loaded after use, inside the slack that was left, and it is loaded only for a photo with a pet and dropped after five idle
minutes. A job's transient memory (the decoded preview, 6 MB as RGBA and 5 MB as RGB, the 5 MB input tensor, the activations) is paid for out of the shared budget: 96 MB per job since named pets (64 before; the extra is DINOv2's activations and the crops) (the measured
extra above is 31 MB plus the decoded preview). The limit is unchanged; `worker.rss_bytes` and `pets.model_loaded` show it live.

*Backfill of 50,000 photos.* Every photo costs its job (claim, a database read of the embedding, the check, the write: about 10 ms), and the detector's 0.2 to 0.4 s (a shared 2 vCPU VPS with one
thread is 1.5 to 2 times slower than this machine) for the photos that hold a pet: at 10 % of the library that is 5,000 photos. **Plan on 30 to 60 minutes** for 50,000 photos
(about 2 hours if half of them are pets), in the background behind uploads, search indexing, text and faces. The pets job starts once a photo has its embedding, so on a library that is still
being embedded for the first time the two overlap and pets finish soon after the embeddings do. `GET /assets/processing` shows `pets.percent`.

*Default on.* The numbers above are why it is on: the cost is a few milliseconds per photo plus a short detector run for a pet photo, the memory is inside the existing limit, and it stores nothing
about people. If a deployment wants none of it, `PETS_ENABLED=0` and nothing runs.

### Cost of named pets

- **Fingerprints:** 140 ms an animal with 2 threads (8-bit DINOv2-small), in the job that finds the animal; the model is 24 MB on disk; it and the detector together are about 90 MB resident (a measured process, 8 MB before loading), loaded on first use and dropped after five idle minutes.
  The job holds 96 MB of the worker's byte budget (was 64). The backfill of animals found earlier is one such job per photo with animals, without a detector run: 5,000 photos with a pet are about 20 to 30 minutes of
  background work (each also downloads and decodes the preview).
- **Proposals and the pets list** are exact (no index): one pass over the owner's free animals of the kind. Measured with 20 pets of 30 animals each and **20,000** free animals of one owner
  (`cargo test -p cyanotype-assets named_pets_tests::the_pets_list -- --ignored --nocapture`, a debug build on a shared machine with Postgres in a container): one pet's proposals **0.13 s**, the pets list
  (with every pet's pending count, one query) **0.67 s**. Twenty thousand free animals is a library of about 150,000 photos with a pet in one in seven; a family library is a tenth of that. A nearest-neighbour
  index was not added: it would have answered one pet in milliseconds and made the list's count (all pets at once) no faster, for an index that costs every insert. If a library outgrows it, the place is
  an HNSW index per `embed_model` as `faces` has (`docs/faces.md`).
- Storage: 1.5 KB an animal (384 half-precision numbers).

## Operating it

- Nothing to install: the detector and DINOv2 are in the worker image and the prompts are in the binary. In a development checkout `just models` (or `scripts/fetch-models.sh`) installs it; without
  the model file the worker says so and the `pets` jobs stay queued.
- Watch: `GET /assets/processing` (`pets`: `scanned`, `total`, `found`, `unfingerprinted`, `queued`, `running`, `failed`, `percent`), the Datadog dashboard (backlog, photos by the check's verdict, job and detector
  time, animals found by kind and photos with a pet the detector could not place), and the metrics below. Failed `pets` jobs show in the failure list and "Retry failed" requeues them.
- Turn off: `PETS_ENABLED=0` for the worker and the assets service. Existing data stays until you delete it (`cyanotype pets delete --apply`).
- A photo with a dog that was not found: look at the check's reading, `SELECT dog_share, cat_share FROM pet_scans WHERE asset_id = ...` (below 0.25 the detector was never asked), and at the detector's
  `pets.unlocated` count (the check said yes, no box was found).

Metrics (`docs/observability.md`): `cyanotype.pets.model_loaded` (gauge), `cyanotype.pets.checked` (tag `verdict`), `cyanotype.pets.duration`, `cyanotype.pets.detector_ms`, `cyanotype.pets.per_photo`,
`cyanotype.pets.found` (tag `kind`), `cyanotype.pets.unlocated`, `cyanotype.pets.fingerprint_ms`, `cyanotype.pets.model_load_ms`, and the `pets` kind of `queue.*` and `jobs.*`.

## Tests

- `apps/web`: `petsText.test.ts` (kinds, counts, the scan), `pages/PetsPage.test.tsx` (off is not found, the switch and its kinds, the link's kind, the scope of a bulk action, save as album, scanning and
  none found, a retry, and the sidebar entry), `Collections.test.tsx` (the row), `searchText.test.ts` and `pages/Search.test.tsx` (the address, the chip and `skip=pets`, off), `FilterSheet.test.tsx`,
  `smartText.test.ts` and `SmartAlbumEditor.test.tsx` (the rule); `e2e/pets-layout.spec.ts` at phone and desktop widths, light and dark.

- `crates/cyanotype-clip`: `zeroshot.rs` unit tests with no model (an embedding on a dog prompt is a dog, one between a dog and a horse splits its share, a wrong-sized embedding is refused, the shipped file
  holds every prompt as a unit vector); `tests/model.rs` with the real models: the committed prompt vectors are the text tower's, and the check finds the dog and the cat and nothing in the 19 other
  fixture photos (well clear of the threshold).
- `crates/cyanotype-pets`: unit tests with no model for the decoding (a box in pixels from fractions, other classes and unsure queries, overlapping and contained boxes of one kind are one animal, a
  weak second box is not a second animal, boxes cut to the picture, at most so many per kind, outputs that do not fit are refused, the input tensor); `tests/model.rs` with the real model on the search
  tests' fixtures (a dog and a cat are found where they are, a horse, a bird, a car, a beach, a pizza and a mountain have none, the same picture at twice the size gives the same boxes);
  `tests/bakeoff` (ignored) is the measurement above.
- `crates/cyanotype-core/tests/pets.rs` (needs `DATABASE_URL`): rows replaced, never duplicated; a newer model's scan takes the older rows with it; nothing stored for a photo being deleted; the kind is data
  (any word, not an empty one, a box has an area); the sweep (only photos with a preview and a search embedding, once, at backfill priority; lost and failed jobs); a new embedding queues the job again;
  progress; the cascade on photo and account deletion; `delete_all` (dry run, one owner, everyone, other jobs untouched); forgetting after an edit; pruning; reading an embedding back.
- `apps/worker/src/pets.rs`: the job against a real database, bucket and models: a dog and a cat the check confirms get boxes inside the picture, running again replaces them; the check decides the kinds
  (a dog photo the check calls a cat has no pets); a pet the detector cannot find is a photo without pets; a photo the check says has no pet is scanned without the preview being read or the detector loaded
  (also when the preview is not in the bucket); a photo without an embedding, and one whose embedding is being made again, are skipped; skips and failure classes; the sweep and the pruning; and the real
  SigLIP 2 embeddings of a dog, a cat, a horse and a car through the whole job.
- Named pets: `crates/cyanotype-pets/src/crop.rs` (the crop: padding, square, kept inside the picture, degenerate boxes), `crates/cyanotype-core/tests/named_pets.rs` (the decisions through a new scan: same boxes,
  a moved animal by its fingerprint, one to one within a kind, another model's numbers not compared, nothing found, an edit forgetting only what nobody decided about, a photo's only animal after a rotate; the
  sweep that fingerprints without finding again; proposals ranked and in tiers, answers changing the next ones, everything left out: another pet's, refused, trashed, Hidden, scanned again, another model,
  another kind, another owner; the pending counts agreeing with the proposals; deletion cascades; `delete_all`), `apps/worker/src/pets.rs` (fingerprints in the job that finds the animal, the same dog twice is
  alike and a cat is not, an end-to-end proposal, animals found before get theirs without being found again and keep their pet, a crop too small has none),
  `apps/assets/src/routes/named_pets_tests.rs` (the switch; every route; making a pet from photos with one, several and no animal; validation; the cap; moving between pets; the list, covers and `pending`;
  rename, hide, delete and merge with owner isolation and audit entries that are counts; "not this pet"; tiers, counts and exclusions; the Hidden view and its unlock; bulk and single answers; a photo's own pet;
  the page's photos, paging and timeline; the scope; Save as album; the `pet` filter, its mistakes and isolation; names in a query, chips, `skip=pet`, hidden, shared and person-owned names; the smart rule;
  a share leak test over every public response, the album and share lists and the recovery manifest; a performance test, ignored), `crates/cyanotype-core/src/manifest.rs` (a pet rule is kept out of the manifest).
- Named pets in the web app: `petsText.test.ts` (names, labels, ids, the plain-words reasons), `components/NamePetSheet.test.tsx` (name and kind, the photo with several animals, the photos left out, one animal),
  `AnimalPicker.test.tsx`, `PetSuggestions.test.tsx` (tiers, one by one and in bulk, optimistic with a rollback, the whole photo), `LightboxPets.test.tsx` (the Info panel's Pets row), `pages/PetPage.test.tsx`
  (rename, hide, cover, merge, delete, "Not this pet", save as album), the "Your pets" row in `PetsPage.test.tsx`, `Library.pets.test.tsx`, the chip, filter sheet and rule tests in `Search.test.tsx`,
  `FilterSheet.test.tsx` and `SmartAlbumEditor.test.tsx`; `e2e/pets-named-layout.spec.ts` at phone and desktop widths, light and dark.
- `apps/assets/src/routes/pets_tests.rs`: the switch (every route a `404` while off, the filter and the words ignored, the facet and the processing entry absent, the scope and the rule refused), the summary
  (counts per kind, covers, scan progress), the Pets view and its timeline (kinds, order, paging, bad input), the trash, hidden and archived rules and the Hidden view, owner isolation, a photo's boxes,
  the search filter combined with dates, favorites, places, ratings, text and people, the typed words and their chips (compounds left alone, `skip=pets`, an explicit filter winning), the facet, "More like
  this", the bulk scope on every verb, smart albums (kinds, all and any, live membership, bad rules, the switch), a share link that carries nothing, and the processing report; `query_parse.rs` (the words and
  their guards, a name first); the purge removing a photo's rows (`routes/trash.rs`); `key_scope_tests.rs` (read scope).
