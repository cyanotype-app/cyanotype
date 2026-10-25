# Moving to another bucket (for example Cloudflare R2)

The bucket is the source of truth: originals, derived images, the daily manifests and the Postgres dumps all live in it. Cyanotype
only speaks the S3 API, so moving to another S3-compatible store is a copy plus a change of the `S3_*` settings. No code changes
and no migration. This page is the runbook, written for Linode Object Storage to Cloudflare R2; the steps are the same for any
other store.

## Before you decide: what changes on R2

| | Linode (today) | R2 |
|---|---|---|
| Egress (every thumbnail, preview and download is a presigned GET from the bucket) | counted against the account's transfer pool | free |
| Storage | flat plan | per GB-month, plus per-request charges (small at this scale) |
| Object versioning | on | **not available** (`GetBucketVersioning` / `PutBucketVersioning` are unimplemented) |
| Retention locks | none | bucket lock rules per prefix |
| CORS policy | cannot be set | can be set (not needed: the map tiles keep going through the assets service) |
| Region | cluster ID (`us-ord-10`) | `auto` |

**What losing versioning means.**

- **Overwrites and deletes are final.** Today a deleted or overwritten object can still be read from an older version; on R2 it is
  gone.
- **"Restore version" in the integrity check has nothing to restore.** The storage code sees that `ListObjectVersions` is
  unsupported and falls back to a plain listing (`Storage::list_versions`), so the other checks keep working.
- **"Delete forever" is a plain delete,** which is what the code already does on an unversioned store.
- **Anything holding the key can delete photos and dumps together.** Two mitigations:
  - A bucket lock rule on `backups/` (below), so a dump cannot be deleted before it is a set number of days old.
  - A second copy elsewhere. The simplest is the old Linode bucket: keep it, and copy R2 into it with `rclone copy` (never `sync`,
    which deletes) on a schedule. Leaving R2 costs nothing, and arriving at Linode is free.

The server stays where it is and reaches R2 over the internet. That adds a little latency to imports and thumbnail jobs. Browsers
fetch photos straight from R2 with presigned URLs, as they do from Linode today.

## 1. Create the bucket and keys (you)

1. **The bucket.** In the Cloudflare dashboard, go to R2, then Create bucket, and name it (for example `cyanotype`). Keep the
   automatic location, or pick a location hint near the server.
2. **The key.** Go to R2, then Manage API tokens, then Create token:
   - Permission: Object Read & Write.
   - Scope: **only this bucket**.
   - Note the Access Key ID, the Secret Access Key and the S3 endpoint, `https://<account-id>.r2.cloudflarestorage.com`.
   - Keep them out of chat, tickets and the repository. They only go into `.env.prod` on the server.
3. **Bucket lock on the dumps (recommended).** Under the bucket's Settings, add a Bucket lock rule:
   - prefix `backups/`;
   - retention **13 days**.

   Keep the retention below `BACKUP_KEEP` days (default 14, one dump a day). The backup job prunes the oldest dump once there are
   more than `BACKUP_KEEP`, and a dump still under lock cannot be deleted, so that night's backup would fail.
4. **A throwaway test bucket** with its own token (same settings), for step 2. Delete both after the test.

## 2. Try the store first (with the test bucket)

Run this from a development machine with the test bucket's values. Never use the real bucket here.

```sh
export S3_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com S3_REGION=auto S3_FORCE_PATH_STYLE=true \
       S3_BUCKET=<test bucket> S3_ACCESS_KEY=<test key id> S3_SECRET_KEY=<test secret>
cargo test -p cyanotype-core --test storage      # multipart, presigned GET/PUT, ranges, deletes (versioning is skipped when absent)
scripts/e2e-import.sh                            # a real import through the CLI and the worker against this bucket
```

What to look for:

- **Uploads (single and multipart) succeed.** The AWS SDK sends checksum headers by default, and R2 accepts only some checksum types.
  A `400` mentioning a checksum is the sign. Report it before going further: the fix is a client setting, not a workaround on the
  bucket.
- **The presigned download keeps its filename** (`presigned_download_is_saved_as_an_attachment_with_the_original_name`).
- **The run takes a reasonable time.** The server is far from R2's storage, so expect slower uploads than on Linode.

## 3. Copy everything (no downtime)

**What is copied.** `rclone sync` copies the current version of every object, which is all Cyanotype reads. Old versions are not
copied: they exist only on Linode, and stay there as long as you keep that bucket.

**How big it is.** Check the size first, since what leaves Linode counts against the account's transfer pool. The Object Storage
bucket page shows it, or run `r2copy size` (below).

**Running the copy.** Run it on the server, in the backup container: it already has rclone and the Linode settings, and its memory
limit (128 MB) is why the buffers below are small. The R2 values go in by name from your shell, so they are never on disk or in the
command line:

```sh
cd ~/cyanotype
export R2_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com R2_BUCKET=<r2 bucket>
read -r -p 'R2 key id: ' R2_ID; read -r -s -p 'R2 secret: ' R2_SECRET; echo; export R2_ID R2_SECRET
r2copy() {   # r2copy <rclone command> [flags]: runs it from the Linode bucket to the R2 bucket
  dc run --rm --no-deps -e R2_ENDPOINT -e R2_BUCKET -e R2_ID -e R2_SECRET --entrypoint sh backup -c '
    export RCLONE_CONFIG_LINODE_TYPE=s3 RCLONE_CONFIG_LINODE_PROVIDER=Other RCLONE_CONFIG_LINODE_ENDPOINT="$S3_ENDPOINT" \
           RCLONE_CONFIG_LINODE_REGION="$S3_REGION" RCLONE_CONFIG_LINODE_ACCESS_KEY_ID="$S3_ACCESS_KEY" \
           RCLONE_CONFIG_LINODE_SECRET_ACCESS_KEY="$S3_SECRET_KEY" RCLONE_CONFIG_LINODE_FORCE_PATH_STYLE=true
    export RCLONE_CONFIG_R2_TYPE=s3 RCLONE_CONFIG_R2_PROVIDER=Cloudflare RCLONE_CONFIG_R2_ENDPOINT="$R2_ENDPOINT" \
           RCLONE_CONFIG_R2_ACCESS_KEY_ID="$R2_ID" RCLONE_CONFIG_R2_SECRET_ACCESS_KEY="$R2_SECRET" RCLONE_CONFIG_R2_NO_CHECK_BUCKET=true
    op=$1; shift
    [ "$op" = size ] && exec rclone size "linode:$S3_BUCKET" "$@"
    exec rclone "$op" "linode:$S3_BUCKET" "r2:$R2_BUCKET" "$@"' sh "$@"
}
r2copy size                                   # how much leaves Linode (the Linode bucket's size)
r2copy sync --transfers 4 --checkers 8 --s3-upload-concurrency 2 --s3-chunk-size 8M --stats 30s --stats-one-line
```

If rclone in the container is stopped for memory (exit code 137), lower `--transfers` to 2.

**Running it again.** The sync can be repeated as often as you like: each run only copies what changed. Run it once now, and once
more at the cutover.

Cloudflare's Super Slurper (R2, then Data Migration) can do the bulk copy from an S3-compatible source instead. The final sync at
the cutover is still rclone.

## 4. Check the copy

```sh
r2copy check --one-way --size-only --checkers 8
```

**What `check` compares.** It reports objects that are missing or have a different size. Hashes are not compared across the two
stores (a multipart object has no MD5 to compare). The hash check is the next step.

**The hash check.** After the cutover, run the integrity check. It reads the originals back from R2 and compares their SHA-256 with
the database: Admin, then Health, then Integrity, then Run now (or wait for the night). Expect no "hash mismatch" and no "missing
original" findings.

## 5. Cut over (a few minutes)

1. Stop writes: `dc stop worker assets auth web`. The gateway shows an error page for those minutes. Postgres and the backup service
   keep running.
2. Run the final sync from step 3, then the check from step 4.
3. In `.env.prod`, replace the Linode values:

   ```
   S3_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
   S3_PUBLIC_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com   # if set; presigned URLs use this host
   S3_REGION=auto
   S3_FORCE_PATH_STYLE=true
   S3_BUCKET=<r2 bucket>
   S3_ACCESS_KEY=<r2 key id>
   S3_SECRET_KEY=<r2 secret>
   CSP_S3_ORIGIN=https://<account-id>.r2.cloudflarestorage.com      # if set; else it is derived from the public endpoint
   ```

   Keep the old block commented out until step 7.
4. `dc up -d`, all services, the backup service included, so its next dump goes to R2.
5. Check:
   - **Datadog:** every service logs `listening`, and there are no `S3` errors.
   - **Browser:** the timeline's thumbnails load. If they do not, look in the console for a CSP `img-src` error: that means
     `CSP_S3_ORIGIN` is wrong.
   - **Photos:** a photo opens, an original downloads, and a share link works in a private window.
   - **Imports:** a new photo through the CLI gets its thumbnail.
   - **Admin, Health:** the backup card shows the newest dump. That is the one copied from Linode until the next night. Run
     `dc run --rm backup now` to make one on R2 straight away.
6. Run the integrity check (step 4), and later a read-only restore drill against R2 (`scripts/restore-drill.sh --source real`,
   with a read-only R2 token: see recovery.md).
7. Leave the Linode bucket untouched for a few weeks. Then delete it, or keep it as the second copy (above).

## Going back

Before anything new is uploaded, put the commented Linode block back in `.env.prod` and `dc up -d`.

Once photos have been added on R2, copy them back first. Use `rclone copy` from R2 to Linode, which adds and never deletes; it is
`r2copy` with the two remotes swapped, run while the Linode block is back in `.env.prod`. Then start the services.
