# Faces in photos

The worker can find the faces in every photo and keep a fingerprint (an *embedding*, 128 numbers) of each. That is the groundwork for a
People page: grouping the fingerprints into people, naming them and showing them is the next piece of work, not this one. Nothing in the API
or the web app reads this data yet.

**It is off by default.** `FACES_ENABLED=1` turns it on (worker and assets service); leave it unset and no job is queued, no model is read and
nothing is stored. A face fingerprint is biometric data about the people in a library, some of whom never agreed to anything: read
"Privacy" below before switching it on.

## Contents

- [What it does](#what-it-does)
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
   faces (`face_scans`), so running the job again never duplicates a face.

### Is the preview enough?

For people it is, with one limit worth knowing. A face has to be at least 32 px in the *preview* to get a fingerprint, and the preview is 1440 px
on its long edge: a face needs a shorter side of about 2.2 % of the photo's long edge. Smaller faces (people far away, big crowds) are found
without a fingerprint or missed. A pass over the original (a 24 MP decode is 100 MB or more, and the budget is 640 MB shared with thumbnails)
would reach smaller faces and was not worth the memory risk for a first version; a face that small is also the one the recognizer is least sure
about (the table below). If the People
page needs it, the way is a second, optional job that decodes the original in strips, not a bigger preview.

## Privacy

- **Off by default** (`FACES_ENABLED`, in `.env.prod`). Switch it on deliberately.
- **Data stays in your database.** Two new tables, `faces` and `face_scans` (migration 0029): box and landmarks as fractions of the preview, a
  score, the fingerprint (`halfvec(128)`), and a `person_id` column that is empty until the grouping step exists. No model is called over
  the network: the models are baked into the worker image and run in the worker process.
- **Deleted with the photo.** A permanent delete (`apps/assets/src/purge.rs`) removes the photo's face rows in the same transaction as
  the photo; the foreign keys cascade from `assets` and from `users` too, so deleting an account removes its faces.
  A rotate or crop forgets the photo's faces (their positions are wrong for the new picture) and scans it again.
- **Not shared.** No API reads these tables yet, and share links, the public pages, downloads, the recovery manifests (`manifest/`) and
  the CLI import never include them. A recovery from the bucket (`cyanotype reindex`) does not bring faces back; they are
  derived data, found again from the previews when the feature is on. Nothing in `apps/assets` reads the tables except the owner's own progress
  counts (`/assets/processing`) and the purge that deletes from them.
- **One command removes everything.** `cyanotype faces delete [--email ADDRESS] --apply` deletes every face, every scan record and every queued or
  finished `faces` job, for one account or for all. It is a dry run (counts only) without `--apply`; it touches nothing else.
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
- `apps/assets`: the processing report (absent while off, owner-scoped on) and the purge removing a photo's faces.
