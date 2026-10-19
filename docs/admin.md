# Administration

Cyanotype is multi-user. One or more accounts are **admins**; everyone else is a **member**. Admins manage who may
join. They never see other people's photos: the admin area deals in accounts, invites and counts, not library content.

This page describes what exists so far: roles, invites, the audit log, storage usage with the accounts page, and per-account limits. It grows with the portal.

## Roles

- `users.role` is `admin` or `member`.
- **Upgrading:** migration 0016 makes the earliest-created account an admin (one row updated). A single-owner instance
  therefore upgrades with its owner as admin and nobody locked out.
- **Fresh install:** the first account to register (in any mode but `closed`) becomes the admin; the check and the
  insert run under one advisory lock, so a race produces exactly one admin.
- `GET /auth/me` reports the role. The web app shows an **Admin** link to admins and treats `/admin` as an unknown page
  for members.
- **The server enforces it.** Admin endpoints (`/auth/admin/*`) use the `AdminUser` extractor
  (`crates/cyanotype-service/src/auth.rs`): it needs a **login session** (an API key is refused with 403, even an
  admin's, so a leaked CLI key cannot reach the portal), reads the role from the database on every request (a demotion
  takes effect at once), and answers 401 without a session and 403 for a member. Any service in the workspace can use it.
- Lost the only admin? Promote an account by SQL (see `docs/deploy.md`, "Upgrading an existing instance").

## Registration modes

`REGISTRATION_MODE` (auth service; `docs/deploy.md`):

| Mode | Who can register |
|---|---|
| `invite` (default) | The first account of an empty instance, freely. Everyone after that only with a valid invite token. |
| `first_user` | The first account only. Invites are ignored. |
| `open` | Anyone. A valid invite token, if given, is still recorded as used. |
| `closed` | Nobody (403). |

`GET /auth/registration` tells the register page the mode and whether the instance still has no account.

## Invites

An admin makes an invite on **Admin > Invites** (`POST /auth/admin/invites`):

- **Single use.** The first registration that presents the token claims it atomically (a row lock), so ten people racing
  with one link register exactly one account. A registration that then fails (the email is taken) rolls the claim back and
  the invite stays usable.
- **Expiry**, 7 days by default, 1 to 90 days.
- **Note**, only the admin sees it (up to 200 characters).
- **Preset limits** (storage, uploads per day, largest file; all optional, positive byte counts) are stored on the invite
  for the limits work to copy onto the new account.
- At most 100 invites may be waiting at once.

The link is `https://<your host>/register?invite=<token>`. The server sends no email: the admin copies the link and sends
it. The register page reads `?invite=` (and, on an invite-only server without one, accepts a pasted link or token), checks
it with `POST /auth/invites/check` and says up front when it was used, revoked or has expired.

**Tokens are stored hashed** (sha256 of 32 random bytes). The plaintext exists only in the response that creates or
re-issues an invite. So "show the link again" means **re-issue**: `POST /auth/admin/invites/{id}/reissue` replaces the
token (the old link stops working immediately) and returns the new one. The expiry is kept unless the invite has already
expired or a new `expires_in_days` is given.

Other actions: list with status (`pending`, `used`, `expired`, `revoked`) and who used each one; **revoke**
(`DELETE /auth/admin/invites/{id}`, final, idempotent, not allowed once used).

## Audit log

`audit_log` records admin actions: when, the actor (id and email, kept after the account is deleted), an action name, the
target and a small JSON detail. Never a token, a password or a photo name. Written so far: `invite.create`,
`invite.reissue`, `invite.revoke` (by the admin) and `invite.use` (by the new account). Services write entries with
`cyanotype_service::audit::record`.

## Usage

Each account has one `user_usage` row (migration 0017) with what it stores:

| Number | What it counts |
|---|---|
| originals, files | files that are not in the trash (every file: the hidden RAW half of a RAW + JPEG pair counts too) |
| trash, trash files | files in the trash, **until they are purged** (a purged file's row is deleted) |
| photos | what the library lists: not trashed, not the hidden half of a pair |
| RAW files | RAW files that are not in the trash (by extension, the same list the pairing uses) |
| thumbnails and previews | the sizes of `thumb.webp`, `preview.webp` and `share.jpg`, **measured**, see below |

What counts toward a quota later is originals plus trash (`stored_bytes`); derived images do not.

**How it is kept current.** By triggers on `assets` (INSERT, DELETE, and UPDATE of the columns that count), not by calls in each
code path. A file reaches `assets` by upload, `reindex`, the CLI, pairing and bulk SQL and leaves by trash, restore, purge, "delete
forever", "empty trash", the janitor's retention purge and account deletion; one trigger sees them all, including a revived
trashed file re-uploaded. The triggers are statement-level over transition tables (one upsert per owner per statement): a row-level version made a
50,000-photo insert twenty times slower (62 s against 3 s, every update of the one row lengthening its version chain inside the
transaction). Measured with them on a 50,000-photo library on a busy shared host: bulk insert 2.6 s (3.8 s without, noise),
trashing all 2.1 s, restoring all 2.7 s, an unrelated `UPDATE` of every row 2.6 s, deleting all 3.7 s. Deleting an account does not leave or create a usage row. Tests drive every one
of those paths and compare the row with the numbers recomputed from `assets`.

**Nightly reconcile.** The worker recomputes every account from `assets` once a day (checked hourly, run when the last run is over
23 hours old, one replica at a time by an advisory lock). Drift is logged per account (`usage counters had drifted`), sent as
`cyanotype.usage.reconcile.drifted_users` / `drift_bytes` (see `docs/observability.md`) and repaired under a lock on the account's row,
re-read there, so a photo arriving meanwhile is neither lost nor reported. Its last result is on the accounts page. It also prunes
`upload_log` rows older than 90 days.

**Derived sizes are measured, not estimated.** The worker records `derived_bytes` (thumbnail + preview) when it renders and
`share_copy_bytes` when it writes a share copy. Photos rendered before this existed have none yet; each reconcile measures up to
`USAGE_MEASURE_PER_RUN` (default 2000) of them with HEAD requests (two or three per photo), so a big library catches up over a few
nights. Until every file is measured the pages say "at least" for that number (`derived_complete` is false).

**Uploads.** `upload_log` has one row (when, how many bytes) per completed upload, written in the same transaction that registers
the file. A re-upload of trashed content is an upload (the bytes crossed the wire). A reindex or an import by SQL is not.
"Last 24 hours / 30 days" are rolling windows over it.

**Where it shows.** Settings > Storage (`GET /assets/usage`): the account's own total, a bar of photos against trash, the breakdown,
uploads in the last 24 hours and 30 days. Admin > Accounts (`GET /assets/admin/accounts`, admin session only): per account the
email, role, join date, last active, photos and RAW files, storage, trash, uploads (24 h, 30 d), albums, shares and API keys, and
the server totals. **Last active** is the latest of the account's last login, last API key use and last upload; there is no
per-request "last seen" because that would be a database write on every request. The page never lists anyone's photos, file names
or album titles (a test checks the response for them).

## Bucket check

Admin > Accounts > **Check bucket size** compares the bucket's real contents with the database. It lists every object and version
(a request per thousand objects, slow and billed on a big library), so it is never automatic: the button queues an `admin_tasks`
row, the worker runs it in the background and keeps the result for the page (which polls while it runs). One at a time; asking
again while one is queued or running returns that one. A task `running` for over two hours is taken to be abandoned and runs
again.

The report gives the bucket's current bytes by kind (originals, derived, manifests, edits, other such as backups), the old
versions the versioned bucket keeps (`noncurrent`), originals under an account the database does not know, and the accounts whose
originals in the bucket differ from the database's originals plus trash. A photo uploaded or deleted while it ran can show once;
run it again before acting on a difference.

Audit entries added by this part: `storage.bucket_check`.

## Limits

Each account may have three limits (migration 0018, columns on `users`). **NULL is unlimited**, and nothing is set by the
migration: the owner and every existing account stay unlimited until an admin sets a limit.

| Limit | Meaning |
|---|---|
| `quota_bytes` | what the account may store: originals **plus trash** (a trashed photo counts until it is purged; thumbnails do not) |
| `daily_upload_bytes` | what it may upload in any rolling 24 hours |
| `max_file_bytes` | the largest single file |

**Who gets what.** An account created from now on starts with the *server default* (Admin > Accounts > "Limits for new
accounts", stored as `default_limits` in `server_state`), with an invite's presets in front of it, field by field: a field the
invite leaves empty takes the default. The first account of an instance, the admin, gets neither and is unlimited. Changing the
default never changes existing accounts. An admin edits any account's limits on its card on the accounts page
(`PUT /assets/admin/accounts/{id}/limits`; a field left out or null is unlimited, zero is refused) and the defaults with
`PUT /assets/admin/default-limits`. Both are audited (`account.limits`, `settings.default_limits`) with the old and new values.

**Enforcement at upload creation** (`POST /assets/check`, so the web uploader and `cyanotype import` alike):

| Case | Status and `code` |
|---|---|
| larger than the largest file | 413 `file_too_large` |
| larger than the whole daily cap (it could never fit) | 413 `file_exceeds_daily_cap` |
| would pass the storage quota | 413 `quota_exceeded` (with `quota_bytes`, `stored_bytes`, `reserved_bytes`, `remaining_bytes`) |
| would pass the daily cap | 429 `daily_cap_exceeded` (with `used_bytes`, `retry_at` and `retry_after_secs`, also as `Retry-After`) |

Bodies are `{"error": "<a sentence a person can read>", "code": "...", ...}`. "Exactly at the limit" is allowed; one byte more is
not. The daily window is rolling: `retry_at` is when enough of the oldest uploads have left the last 24 hours for this file to
fit. Re-uploading content that is in the trash revives it and adds nothing to what is stored, so the quota does not refuse it
(the daily cap still counts the bytes).

**Concurrent uploads.** Creation runs under a per-account advisory lock, and what an account has declared but not finished yet
(its `uploads` rows, at their declared size) is *reserved* against the quota and the daily cap, so uploads racing each other
cannot each fit while together passing a limit (a test fires twelve at once against a quota that fits three). A reservation
lasts while the upload shows signs of life; one idle for 24 hours stops counting (the janitor aborts it after 7 days).

**Parts must match what was declared.** The size an upload may have is what it declared and had checked. The proxy
(`PUT /assets/uploads/{id}/parts/{n}`) accepts only a part of exactly the layout's length, so it cannot exceed it. Presigned
direct parts are signed for their exact length (a store that verifies signatures against the real request, such as AWS S3,
refuses any other; the SeaweedFS the tests use does not, so that is not shown by a test), and `POST /assets/complete` lists the
parts that reached storage and, if they add up to more than was declared, aborts the multipart upload, deletes the pending row
and answers 413 `upload_larger_than_declared` before the object exists. A test uploads 500 bytes against a declared 10.

**What users see.** Settings > Storage shows each limit as a meter with its percentage, and warns at 80 % ("You have used 82% of
your storage"), 95 % ("almost full") and when full; it says when the first part of the daily window frees up. The web upload tray
shows the limits (`Storage 4.0 GB of 5.0 GB · last 24 h ...`), the same warnings, and, when the quota or the daily cap refuses a
file, **stops**: the files not started are marked "Not started", the reason is shown with, for the daily cap, when uploads can
resume, and "Try again" queues them again. A single file over the largest-file limit fails alone and the rest carry on.
`cyanotype import` does the same (see `docs/import.md`).

Metric: `cyanotype.uploads.refused{reason}` counts refusals (`quota_exceeded`, `daily_cap_exceeded`, `file_too_large`,
`file_exceeds_daily_cap`, `larger_than_declared`).
