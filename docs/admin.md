# Administration

Cyanotype is multi-user. One or more accounts are **admins**; everyone else is a **member**. Admins manage who may
join. They never see other people's photos: the admin area deals in accounts, invites and counts, not library content.

This page describes the whole portal: roles, invites, storage usage and limits, account actions, sessions, the health card and the audit log.

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

## Two-factor authentication

People turn it on themselves in **Settings > Security**: scan a QR code (or type the key) into an authenticator app, confirm with a
6-digit code, and save the ten single-use recovery codes (shown once, with a download). Sign-in then asks for a code after the
password; a recovery code works in its place, once. Turning it off needs the password and a code. TOTP is RFC 6238: HMAC-SHA1, 30
second steps, 6 digits, one step of clock drift either side; a code is accepted once.

**Admin > Security** shows:

- whether the server key is set. Without `AUTH_TOTP_KEY` ([`deploy.md`](deploy.md)) two-factor authentication is switched off and the
  page says so in red; accounts that already have it cannot sign in until the key is back (the check fails closed, never open);
- **Require two-factor authentication for every account.** You need it on your own account first. Accounts without it are sent to
  Settings > Security right after they sign in and can do nothing else (every service refuses their session, not only the web app)
  until they have enrolled. Live sessions are caught at once. Not enforced while the server has no key;
- who has it on (and how many recovery codes they have left; never a secret), and **Reset** for someone who lost their phone and
  their codes: it removes their 2FA, signs them out everywhere and is written to the audit log (`account.2fa_reset`). Requiring
  or releasing it is logged too (`2fa.require`, `2fa.unrequire`).

How it is protected: secrets are sealed with AES-256-GCM under a key derived from `AUTH_TOTP_KEY` (bound to the account), recovery
codes are stored only as an HMAC under another derived key, so a copy of the database alone cannot be brute-forced. Wrong codes are
throttled per account (five in a row lock the second factor for 15 minutes, even against the right code) and per client address.
A correct password gives only a five-minute challenge, never a session.

## Audit log

`audit_log` records admin actions: when, the actor (id and email, kept after the account is deleted), an action name, the
target and a small JSON detail. Never a token, a password or a photo name. Services and the worker write entries with
`cyanotype_core::audit::record` (also reachable as `cyanotype_service::audit`). The actions, as the audit page words them:

| Action | When |
|---|---|
| `invite.create`, `invite.reissue`, `invite.revoke` | an admin, on an invite; `invite.use` is by the new account |
| `account.limits`, `settings.default_limits` | limits edited, with the old and new values |
| `account.disable`, `account.enable` | an account disabled or enabled |
| `account.role` | promoted or demoted (`from`, `to`) |
| `account.reset_link` | a reset link made (never the link); `account.password_reset` is by the person who used it |
| `account.sessions_revoked` | an admin signed an account out everywhere |
| `account.delete.requested`, `account.delete.finished` | the two ends of an account deletion (the second is written by the worker, with counts) |
| `storage.bucket_check`, `jobs.retry` | a bucket check started; failed jobs retried (kind and count) |

**Admin > Audit log** (`GET /auth/admin/audit`, admins only) lists them newest first and filters by action (the start of the
name, so `account.` is every account action), by who did it (part of the email), by what it was about (an id, or part of an email
in the entry) and by date, and pages backwards with a cursor (`before`).

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

## Account actions

On each account's card on Admin > Accounts, under **Manage** (all of them admin sessions only, audited, enforced by the server):

- **Disable / enable** (`POST /auth/admin/users/{id}/disable|enable`, `users.disabled_at`, migration 0019). A disabled account
  cannot sign in (a correct password gets 403 `account_disabled`; a wrong one the usual 401, so the message reveals nothing to
  someone guessing), its sessions are deleted when it is disabled, and every service refuses its API keys while it is disabled
  (the keys are kept, so enabling restores the CLI; a disabled account's keys are not stamped as used). The check is in the shared
  auth extractors, so assets, auth and the admin endpoints all honour it. Public share links of a disabled account keep working;
  delete the account (or its shares) to stop them.
- **Reset password** (`POST /auth/admin/users/{id}/reset-link`): makes a **one-time link** the admin copies and sends (no email is
  sent). It is stored hashed, shown once, works once, expires after 24 hours and is replaced by the next link made for the same
  account. The link opens `/reset-password?token=...` (no sign-in), which checks it (`POST /auth/reset-password/check`) and sets the
  new password (`POST /auth/reset-password`, throttled with login and register). Using it signs the account out everywhere. A
  disabled account cannot use one.
- **Make admin / Remove admin** (`POST /auth/admin/users/{id}/role`).
- **Sessions** (`GET|DELETE /auth/admin/users/{id}/sessions`): the account's active sessions (when they started and expire, never
  a token) and **Sign out everywhere**. People see their own at Settings > Sessions (`GET /auth/sessions`, `POST
  /auth/sessions/revoke-others`, `/revoke-all`, `DELETE /auth/sessions/{id}`; sessions only, not API keys).
- **Delete account** (`POST /auth/admin/users/{id}/delete` with `confirm_email`): two steps in the web app (open the panel, then
  type the account's email; the server checks it again). The account is disabled and signed out at once and a `delete_account` task
  is queued; the worker then marks every file as being purged (so no new thumbnails are written for them) and waits for jobs
  already running on them (up to 10 minutes, else the task fails and Delete again retries), aborts the unfinished uploads,
  permanently deletes **every version** of everything under `originals/{id}/`, `derived/{id}/`, `manifests/{id}/` and `edits/{id}/`
  (the same removal as "delete forever"), and finally deletes the account row, which removes its files, albums, shares, keys, usage
  and log rows by cascade. Each step can run again, and the objects go before the rows, so a failure never leaves objects with no
  record of whose they are. Tests delete one of two accounts and check the other's objects (every version) and rows are untouched.
  While it runs the card says so; a failure shows its reason with a way to retry.

**Guards**, enforced on the server under one advisory lock: nobody disables or deletes **themselves**; the **last active
administrator** (a disabled admin does not count) can be neither demoted, disabled nor deleted (two admins demoting each other at
the same moment leave exactly one); an account being deleted cannot be enabled.

## Health

Admin > Health (`GET /assets/admin/health`, refreshed every 15 seconds) shows only what the stack already has, with no call to
Datadog:

- **Job queue** by kind (waiting, running, failed, how long the oldest has waited), a warning when jobs are waiting and none has
  finished for a while, the latest failed jobs with their error text (never a file name or owner), and **Retry all failed jobs**
  or per kind (`POST /assets/admin/jobs/retry`, audited), which requeues the failures of every account with a fresh set of
  attempts.
- **Worker**: its resident memory and decode-budget use. The worker writes the values its DogStatsD gauges already carry (and its
  version) to `server_state` (`worker:<host>`) once a minute; one not heard from for 5 minutes shows as not reporting.
- **Backup**: the newest dump under `backups/postgres/` in the bucket (`BACKUP_PREFIX` if the backup service sets another), its
  size and how many are kept; a warning past 36 hours; an error line if the bucket cannot be listed (the rest of the card still
  loads).
- **Sizes**: the database (`pg_database_size`), what the accounts store, thumbnails and, from the latest bucket check, the bucket.
- **Version**: the label the services run under (`DD_VERSION`, which production sets to the image tag, `sha-<commit>`) for the
  assets service and each worker.
- **Storage integrity**: the report of the nightly check below.

## Storage integrity

The bucket is the source of truth, so the worker compares it with the database every night (`INTEGRITY_CHECK_HOUR`, UTC, default
03:00; `docs/deploy.md` lists the settings) and Admin > Health shows the result. **Run check now** queues the same check at any time
(`POST /assets/admin/integrity/run`, audited; one at a time). A check lists `originals/` and `derived/` page by page (a request per
thousand objects), asks the store about the few rows it did not see, and reads a rolling sample of originals back. It runs behind
uploads: the reading pauses while thumbnails or metadata are queued, uses one stream at a time and never holds a file in memory.

| Finding | What it means | Actions |
|---|---|---|
| **Missing original** | A photo's row exists but its object is gone (or hidden behind a delete marker). Photos in the trash and photos being deleted for good are not checked. | **Restore from a previous version** (offered unless the bucket holds none; it reads the earlier versions, picks the one whose sha256 is the stored one and makes it current again), **Move to trash**, **Mark lost** |
| **Untracked object** | An object under `originals/` that no photo points at: no row, an extra copy of indexed content under another key, or a key that is not an original's. Objects younger than 10 minutes, uploads in flight, accounts being deleted and everything outside `originals/` (manifests, `edits/`, `derived/`) are ignored. | **Restore as photo** (the same import `cyanotype reindex --apply` does for one object: name and times from the object's metadata and the owner's newest manifest, the metadata and thumbnail jobs, the photo's `edits/` object; not offered when the owner has no account or the content is already indexed), **Delete object** (every version, for good: type `delete` to confirm), **Leave it** |
| **Checksum mismatch** | The object's size differs from the row's (seen for free in the listing), or its bytes no longer hash to the stored sha256. Both hashes are shown. | **Restore from a previous version** (as above, only a version that matches), **Leave it** |

Missing thumbnails and previews are not findings: the existing `thumbnail` job is queued again by itself and the run counts it
("thumbnails queued to be made again"). A photo whose thumbnail job failed is left to the failed-jobs list above.

**The checksum sample.** Reading every original every night is out of the question on a small server (25 GB would be hours of
traffic), so each run re-reads the photos verified longest ago, never-verified first, until it has read `INTEGRITY_VERIFY_MB`
(default 2048) or `INTEGRITY_MAX_MINUTES` has passed. Each photo that was read gets `assets.verified_at`; the card says how much of the
library has been read at least once. At about 2 GB a night a 25 GB library is covered in about two weeks. A file larger than the
budget is still read when it is the first of a run, so it cannot be skipped for good. `INTEGRITY_VERIFY_MB=0` turns the sample off
(the listing checks stay). A mismatch is re-read on every later run until the file hashes right again, and then closes itself.

Every action is admin-only (a login session, never an API key), idempotent (doing it again changes nothing and says "Already done")
and written to the audit log with the finding's kind and key (the key holds the owner and the content hash, never a file name). A
finding a later run no longer sees closes itself; **Mark lost** and **Leave it** keep a finding true but quiet: it is not counted
and not raised again. Deleting an object also needs `"confirm": "delete"` in the API call.

The worker reports `cyanotype.integrity.findings_open` (per kind) and `cyanotype.integrity.last_run_age_seconds`; two monitors
in `infra/datadog/monitors/` watch them (`docs/observability.md`).
