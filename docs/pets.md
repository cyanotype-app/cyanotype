# Dogs and cats in photos

The worker finds the dogs and cats in every photo, so the owner can list them (a **Pets** view), search for them ("dogs", "cat on the sofa", a
pets filter, a smart-album rule) and select them for bulk actions. For now a dog or a cat is just a *pet*: the photos are found, not the animals told apart.
Boxes are stored for each animal so that a later feature (a page per pet, seeded by tagging) can crop them, but nothing here identifies an individual.

**It is on by default** and costs very little: the search model's own embedding of a photo says which photos hold a dog or a cat (microseconds each, no
photo read), and a small detector draws the boxes on those few. It stores nothing about people. `PETS_ENABLED=0` turns it off (the worker and the assets
service must agree): no job is queued, nothing is read, the Pets routes answer `404` as if they did not exist and the search words stay words.

## Contents

- [What it does](#what-it-does)
- [The check, and what the numbers said](#the-check-and-what-the-numbers-said)
- [In search](#in-search)
- [In the API](#in-the-api)
- [Privacy](#privacy)
- [Models, licences and provenance](#models-licences-and-provenance)
- [Bake-off](#bake-off)
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
   only, the strongest one if it reaches 0.10 and further ones only if they are surer (0.25) and not the same animal again. While it runs it holds 64 MB
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
| `GET /assets/{id}/pets` | `{items: [{id, kind, box: [x, y, w, h], score}], scanned}`: the animals found in one photo, strongest first; `scanned` is false while the photo has not been looked at (`items` is empty either way). For the viewer's boxes and, later, a pet's crop. |
| `GET /assets/search?pets=any\|dog\|cat` | The filter above. `parsed.chips[]` has `{kind: "pets", label: "Dogs" \| "Cats" \| "Pets", matched, pets: "dog" \| "cat" \| "any"}` when words became it. `skip=pets` turns the reading of words off. |
| `GET /assets/search/facets` | `pets: [{value: "any" \| "dog" \| "cat", count}]` while Pets is on. |
| bulk requests: `{"scope": {"pets": "any\|dog\|cat"}, "except": [...]}` | Like `{"person": id}` and `{"album": id}`: the photos of the Pets view for that kind, resolved inside the request's transaction (favorite, rating, tags, hide, archive, trash, restore, edits, albums, shares, zip). A `search` scope with a `pets` filter is **refused (404) while Pets is off**, never read as "no filter": a bulk trash must not widen to the whole library because a switch was flipped. |
| smart albums: `{"field": "pets", "kind": "any\|dog\|cat"}` | "A dog, a cat or either was found in the photo" (the rule editor's *Pets* rule). Saving one while Pets is off is a `404` and a saved rule matches no photo while it is off, in the album, its counts, its bulk scope and its share link (never "no rule", which would be every photo). The album keeps only the kind. |
| `GET /assets/processing` | has a `pets` entry while the feature is on: `{scanned, total, found, queued, running, failed, model, percent}`; a failed `pets` job is also in the failure list and "Retry failed" requeues it. |

Pick names for a UI from these: the Pets entry of the library menu is shown when `status.enabled`; the page is `/assets/pets` for the header and the kind tabs (the counts and
covers), `/assets/pets/photos` and `/assets/pets/timeline` for the grid (the library's own timeline component), bulk actions through `{"pets": kind}`, "Search with filters"
through `pets=`; the viewer's boxes come from `/assets/{id}/pets`.

## Privacy

- **It finds animals, not people.** `pet_detections` and `pet_scans` hold a kind, a box, a score and two shares per photo: no fingerprint, no face and no name. A box that happens to
  contain a person's face in the middle of a dog's picture is a box.
- **Deleted with the photo.** A permanent delete (`apps/assets/src/purge.rs`) removes the photo's rows in the same transaction as the photo; the foreign keys cascade from
  `assets` and from `users` too, so deleting an account removes its detections.
- **Owner-only, never shared.** Share links, the public pages, downloads, the recovery manifests and the CLI import never include detections or boxes (a test shares photos
  with dogs, and a smart album whose rule is "a dog", and searches every public response and the owner's share responses for the kinds, boxes, scores and ids). The routes are for
  the signed-in owner, and a photo in the Hidden view is invisible to a session that has not unlocked it.
- **Derived data, not backed up as such.** The detections are not in the recovery manifests (`docs/recovery.md`): they are found again by a rescan, which the worker queues by itself once
  the photos have their embeddings again. A smart album's `pets` rule is only a kind and travels with the album's rules.
- **One command removes everything.** `cyanotype pets delete [--email ADDRESS] --apply` deletes every detection, every scan record and every queued or finished `pets` job, for one
  account or for all; without `--apply` it only counts. Switch Pets off and restart the worker first, or the sweep finds the photos again. Run it where `migrate` runs
  (`docs/deploy.md`: `dc run --rm reindex pets delete --apply`).

## Models, licences and provenance

The rule (as for faces and text): only models whose weights may be redistributed inside our Docker images. The licence texts were read from the model repositories' own files at the
pinned commits; the repository itself is AGPL-3.0, so AGPL weights would have been allowed but none was needed.

| Model | Used for | Licence | Source (primary) |
| --- | --- | --- | --- |
| **D-FINE-N** (`dfine-n.onnx`, 15 MB, COCO only, 4 M parameters, 42.8 AP) | the boxes | **Apache 2.0** (Yansong Peng et al., `Peterande/D-FINE`) | the ONNX export that Hugging Face's transformers.js team published in `onnx-community/dfine_n_coco-ONNX` at commit `380d2839c327efaf65dd0fe0c2c10ab7fadd5473` (2025-04-30), `onnx/model.onnx`, from `ustc-community/dfine_n_coco` (the Hugging Face port, whose model card states Apache 2.0). Neither Hugging Face repository has a licence file, so the licence text shipped next to the weights is the one in `Peterande/D-FINE` at commit `956d1709314c2c6a4df6f34de232054578a7449f` (2026-08-19, `LICENSE`, Apache 2.0, which its README names for the code and the released weights) |
| **SigLIP 2 base** (the search model, 224 px) | the check: its image embedding is read against prompts embedded with its text tower | **Apache 2.0** (Google) | already in the worker and assets images for search (`models/siglip2.lock`); the prompt vectors are in the repository |

Both are pinned by exact URL and sha256 in `models/pets.lock` (and `models/siglip2.lock`), fetched by `scripts/fetch-models.sh`, verified, and installed with the licence text
(`LICENSE-dfine.txt`) in the worker image under `/opt/cyanotype/models/pets/`, as Apache 2.0 requires. Both are older than 14 days.

**A caveat the licence files cannot settle.** The licences cover the weights and files in those repositories. D-FINE was trained on COCO (images from Flickr under several licences,
annotations CC BY 4.0) with an ImageNet-pretrained backbone, and SigLIP 2 on web data; those datasets have their own terms, which we cannot verify or grant. D-FINE's weights are
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
minutes. A job's transient memory (the decoded preview, 6 MB as RGBA and 5 MB as RGB, the 5 MB input tensor, the activations) is paid for out of the shared budget: 64 MB per job (the measured
extra above is 31 MB plus the decoded preview). The limit is unchanged; `worker.rss_bytes` and `pets.model_loaded` show it live.

*Backfill of 50,000 photos.* Every photo costs its job (claim, a database read of the embedding, the check, the write: about 10 ms), and the detector's 0.2 to 0.4 s (a shared 2 vCPU VPS with one
thread is 1.5 to 2 times slower than this machine) for the photos that hold a pet: at 10 % of the library that is 5,000 photos. **Plan on 30 to 60 minutes** for 50,000 photos
(about 2 hours if half of them are pets), in the background behind uploads, search indexing, text and faces. The pets job starts once a photo has its embedding, so on a library that is still
being embedded for the first time the two overlap and pets finish soon after the embeddings do. `GET /assets/processing` shows `pets.percent`.

*Default on.* The numbers above are why it is on: the cost is a few milliseconds per photo plus a short detector run for a pet photo, the memory is inside the existing limit, and it stores nothing
about people. If a deployment wants none of it, `PETS_ENABLED=0` and nothing runs.

## Operating it

- Nothing to install: the detector is in the worker image and the prompts are in the binary. In a development checkout `just models` (or `scripts/fetch-models.sh`) installs it; without
  the model file the worker says so and the `pets` jobs stay queued.
- Watch: `GET /assets/processing` (`pets`: `scanned`, `total`, `found`, `queued`, `running`, `failed`, `percent`), the Datadog dashboard (backlog, photos by the check's verdict, job and detector
  time, animals found by kind and photos with a pet the detector could not place), and the metrics below. Failed `pets` jobs show in the failure list and "Retry failed" requeues them.
- Turn off: `PETS_ENABLED=0` for the worker and the assets service. Existing data stays until you delete it (`cyanotype pets delete --apply`).
- A photo with a dog that was not found: look at the check's reading, `SELECT dog_share, cat_share FROM pet_scans WHERE asset_id = ...` (below 0.25 the detector was never asked), and at the detector's
  `pets.unlocated` count (the check said yes, no box was found).

Metrics (`docs/observability.md`): `cyanotype.pets.model_loaded` (gauge), `cyanotype.pets.checked` (tag `verdict`), `cyanotype.pets.duration`, `cyanotype.pets.detector_ms`, `cyanotype.pets.per_photo`,
`cyanotype.pets.found` (tag `kind`), `cyanotype.pets.unlocated`, `cyanotype.pets.model_load_ms`, and the `pets` kind of `queue.*` and `jobs.*`.

## Tests

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
- `apps/assets/src/routes/pets_tests.rs`: the switch (every route a `404` while off, the filter and the words ignored, the facet and the processing entry absent, the scope and the rule refused), the summary
  (counts per kind, covers, scan progress), the Pets view and its timeline (kinds, order, paging, bad input), the trash, hidden and archived rules and the Hidden view, owner isolation, a photo's boxes,
  the search filter combined with dates, favorites, places, ratings, text and people, the typed words and their chips (compounds left alone, `skip=pets`, an explicit filter winning), the facet, "More like
  this", the bulk scope on every verb, smart albums (kinds, all and any, live membership, bad rules, the switch), a share link that carries nothing, and the processing report; `query_parse.rs` (the words and
  their guards, a name first); the purge removing a photo's rows (`routes/trash.rs`); `key_scope_tests.rs` (read scope).
