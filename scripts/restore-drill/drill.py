#!/usr/bin/env python3
"""Seeding, snapshots and the report of the restore drill (scripts/restore-drill.sh drives this; stdlib only).

  drill.py seed      --state FILE        make a realistic library through the gateway's real APIs
  drill.py settle    --state FILE        wait for the worker (thumbnails, metadata, embeddings, geocode, bursts)
  drill.py snapshot  --state FILE --out FILE [--login-as-original]
  drill.py report    --before F --path-a F --path-b F --timings F --out F [...]

Everything secret (the throwaway passwords) lives in the state file inside the drill's temp directory and is never printed.
"""
import argparse
import hashlib
import http.cookiejar
import json
import os
import re
import secrets
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fixtures  # noqa: E402
import s3  # noqa: E402

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def log(msg):
    print(f"[drill] {msg}", file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------------------------------- HTTP


class Client:
    def __init__(self, base):
        self.base = base.rstrip("/")
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))
        self.bearer = None

    def request(self, method, path, body=None, raw=None, headers=None, url=None):
        h = dict(headers or {})
        data = raw
        if body is not None:
            data = json.dumps(body).encode()
            h["content-type"] = "application/json"
        if self.bearer:
            h["authorization"] = f"Bearer {self.bearer}"
        req = urllib.request.Request(url or self.base + path, data=data, method=method, headers=h)
        try:
            with self.opener.open(req, timeout=60) as r:
                payload, status, rh = r.read(), r.status, r.headers
        except urllib.error.HTTPError as e:
            payload, status, rh = e.read(), e.code, e.headers
        try:
            parsed = json.loads(payload) if payload else None
        except ValueError:
            parsed = payload
        return status, parsed, rh

    def ok(self, method, path, body=None, expect=(200, 201, 204), **kw):
        status, parsed, rh = self.request(method, path, body, **kw)
        if status not in expect:
            raise SystemExit(f"{method} {path} -> HTTP {status}: {str(parsed)[:300]}")
        return parsed


# ---------------------------------------------------------------------------------------------------- database


def psql(sql):
    """Run SQL in the drill's Postgres (DRILL_PSQL is the full command, set by the shell script); tab-separated rows."""
    cmd = shlex.split(os.environ["DRILL_PSQL"]) + ["-X", "-A", "-t", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-c", sql]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"psql failed: {r.stderr.strip()[:400]}")
    return [line.split("\t") for line in r.stdout.splitlines() if line != ""]


def psql_one(sql):
    rows = psql(sql)
    return rows[0][0] if rows else None


# ---------------------------------------------------------------------------------------------------- seed

# Capture places: all of them are big cities in the bundled places dataset, so the geocode job finds a name.
LISBON = (38.7223, -9.1393)
TOKYO = (35.6762, 139.6503)
ZERMATT = (46.0207, 7.7491)
PARIS = (48.8566, 2.3522)
NYC = (40.7128, -74.0060)

# (fixture, uploaded name, taken, gps, camera) for the main library
ALICE_PHOTOS = [
    ("beach", "beach.jpg", "2024:06:15 10:00:00", LISBON, ("DrillCam", "Alpha 1")),
    ("mountain", "mountain.jpg", "2024:07:02 08:30:00", ZERMATT, ("DrillCam", "Alpha 1")),
    ("city_night", "city_night.jpg", "2023:12:31 23:50:00", TOKYO, ("DrillCam", "Alpha 1")),
    ("forest", "forest.jpg", "2023:05:05 14:00:00", None, ("DrillCam", "Alpha 1")),
    ("dog", "dog.jpg", "2024:03:09 16:20:00", None, ("DrillPhone", "P1")),
    ("cat", "cat.jpg", "2024:03:10 09:05:00", None, ("DrillPhone", "P1")),
    ("pizza", "pizza.jpg", None, None, None),  # no EXIF at all: the timeline falls back to the file's mtime
    ("coffee", "coffee.jpg", "2024:09:01 07:45:00", PARIS, ("DrillPhone", "P1")),  # goes to the trash
    ("horse", "horse.jpg", "2022:08:20 12:00:00", None, ("DrillCam", "Alpha 1")),  # gets edited
    ("sailboat", "sailboat.jpg", "2024:06:16 17:10:00", LISBON, ("DrillCam", "Alpha 1")),
]
BURST = ("bird", "2024:08:10 14:00:00", ("DrillCam", "Burst 9"))  # three near-identical shots, one second apart
BOB_PHOTOS = [
    ("train", "train.jpg", "2024:04:01 09:00:00", NYC, ("DrillPhone", "B2")),
    ("sunset", "sunset.jpg", "2024:04:02 19:30:00", None, ("DrillPhone", "B2")),
    ("guitar", "guitar.jpg", "2024:04:03 20:00:00", None, ("DrillPhone", "B2")),
]

RAW_SAMPLE = (
    "DSC_0451.NEF",
    "https://raw.pixls.us/data/Nikon/1%20J2/DSC_0451.NEF",
    "81051ee36135883acea55bd1e609d6d31212c0bf03640b7ae9fe177a1ca51219",
)


def sha256_hex(b):
    return hashlib.sha256(b).hexdigest()


def upload(c, data, name, ctype, mtime="2024-10-01T12:00:00Z"):
    chk = c.ok(
        "POST",
        "/assets/check",
        {
            "sha256": sha256_hex(data),
            "size_bytes": len(data),
            "filename": name,
            "content_type": ctype,
            "file_modified_at": mtime,
        },
    )
    if chk.get("status") != "upload":
        return chk.get("asset_id") or chk.get("id")
    # The drill's files are far below one part (16 MiB): a single presigned PUT straight to the bucket, like the CLI does.
    parts = []
    for p in chk["parts"]:
        status, _body, rh = c.request("PUT", None, raw=data, url=p["url"])
        if status not in (200, 201):
            raise SystemExit(f"part upload of {name} failed: HTTP {status}")
        parts.append({"number": p["number"], "etag": rh.get("ETag")})
    done = c.ok("POST", "/assets/complete", {"upload_id": chk["upload_id"], "parts": parts})
    return done["asset_id"]


def fetch_raw(cache):
    """The CC0 RAW sample (sha256-pinned), cached under `cache`. None if it cannot be had (the pair is then skipped)."""
    name, url, want = RAW_SAMPLE
    os.makedirs(cache, exist_ok=True)
    path = os.path.join(cache, name)
    if os.path.exists(path) and sha256_hex(fixtures.read(path)) == want:
        return path
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers={"user-agent": "cyanotype-restore-drill/1"}), timeout=120) as r:
            data = r.read()
    except Exception as e:  # noqa: BLE001
        log(f"RAW sample unavailable ({e}); the RAW+JPEG pair is skipped")
        return None
    if sha256_hex(data) != want:
        log("RAW sample checksum mismatch; the RAW+JPEG pair is skipped")
        return None
    with open(path, "wb") as f:
        f.write(data)
    return path


def random_password():
    return secrets.token_urlsafe(18)


def cmd_seed(a):
    base = os.environ["DRILL_BASE_URL"]
    state = {"base": base}
    alice, bob = Client(base), Client(base)
    pw_a, pw_b = random_password(), random_password()
    state["users"] = {
        "alice": {"email": "alice@drill.invalid", "password": pw_a},
        "bob": {"email": "bob@drill.invalid", "password": pw_b},
    }

    log("registering the admin (first account)")
    alice.ok("POST", "/auth/register", {"email": "alice@drill.invalid", "password": pw_a})
    me = alice.ok("GET", "/auth/me")
    state["users"]["alice"]["id"] = me["id"]
    log(f"admin role: {me.get('role')}")

    log("server defaults and an invite for bob with its own limits")
    alice.ok("PUT", "/assets/admin/default-limits", {"quota_bytes": 10 * 2**30, "daily_upload_bytes": 2 * 2**30, "max_file_bytes": 512 * 2**20})
    inv = alice.ok("POST", "/auth/admin/invites", {"note": "drill: bob", "expires_in_days": 7, "quota_bytes": 5 * 2**30})
    bob.ok("POST", "/auth/register", {"email": "bob@drill.invalid", "password": pw_b, "invite": inv["token"]})
    state["users"]["bob"]["id"] = bob.ok("GET", "/auth/me")["id"]
    alice.ok(
        "PUT",
        f"/assets/admin/accounts/{state['users']['bob']['id']}/limits",
        {"quota_bytes": 6 * 2**30, "daily_upload_bytes": 1 * 2**30, "max_file_bytes": 256 * 2**20},
    )
    # A second invite stays pending, and an unused reset link, so their fate is visible too.
    alice.ok("POST", "/auth/admin/invites", {"note": "drill: pending invite", "expires_in_days": 14, "daily_upload_bytes": 2**30})
    alice.ok("POST", f"/auth/admin/users/{state['users']['bob']['id']}/reset-link", {})

    log("API keys")
    k_alice = alice.ok("POST", "/auth/api-keys", {"name": "drill cli (alice)"})
    k_bob = bob.ok("POST", "/auth/api-keys", {"name": "drill cli (bob)"})
    state["users"]["alice"]["api_key"] = k_alice.get("key") or k_alice.get("token")
    state["users"]["bob"]["api_key"] = k_bob.get("key") or k_bob.get("token")

    log("photos")
    images = fixtures.fixture_dir(REPO)
    ids = {}
    n = 0
    for fx, name, taken, gps, cam in ALICE_PHOTOS:
        jpg = fixtures.read(os.path.join(images, fx + ".jpg"))
        if taken:
            jpg = fixtures.with_exif(jpg, fixtures.exif_app1(cam[0], cam[1], taken, gps))
        ids[name] = upload(alice, jpg, name, "image/jpeg", mtime=f"2024-10-{1 + n % 20:02d}T12:00:00Z")
        n += 1
    fx, taken, cam = BURST
    base_jpg = fixtures.read(os.path.join(images, fx + ".jpg"))
    for i in range(3):
        h, m, s = taken.split(" ")[1].split(":")
        t = f"{taken.split(' ')[0]} {h}:{m}:{int(s) + i:02d}"
        jpg = fixtures.with_exif(base_jpg, fixtures.exif_app1(cam[0], cam[1], t), tail=f"\ndrill-burst-{i}".encode())
        name = f"burst_{i + 1}.jpg"
        ids[name] = upload(alice, jpg, name, "image/jpeg")
    raw_path = fetch_raw(a.cache)
    if raw_path:
        info = fixtures.tiff_exif(raw_path)
        if info:
            raw_name = "IMG_0001.NEF"
            ids[raw_name] = upload(alice, fixtures.read(raw_path), raw_name, "image/x-nikon-nef")
            jpg = fixtures.with_exif(
                fixtures.read(os.path.join(images, "airplane.jpg")), fixtures.exif_app1(info[0], info[1], info[2])
            )
            ids["IMG_0001.JPG"] = upload(alice, jpg, "IMG_0001.JPG", "image/jpeg")
        else:
            log("could not read EXIF from the RAW sample; no pair")
    for fx, name, taken, gps, cam in BOB_PHOTOS:
        jpg = fixtures.with_exif(fixtures.read(os.path.join(images, fx + ".jpg")), fixtures.exif_app1(cam[0], cam[1], taken, gps))
        ids["bob/" + name] = upload(bob, jpg, name, "image/jpeg")
    state["assets"] = ids
    log(f"uploaded {len(ids)} files")

    # Wait until the photos have thumbnails and metadata (and RAW pairing, which needs both halves' EXIF), then use them.
    wait_worker(state, quick=True)

    log("albums, share links, favorites, trash")
    summer = alice.ok(
        "POST",
        "/assets/albums",
        {"title": "Summer 2024", "description": "Synthetic drill album", "asset_ids": [ids["beach.jpg"], ids["mountain.jpg"], ids["horse.jpg"], ids["sailboat.jpg"]]},
    )
    pets = alice.ok("POST", "/assets/albums", {"title": "Pets", "asset_ids": [ids["dog.jpg"], ids["cat.jpg"]]})
    alice.ok("PATCH", f"/assets/albums/{summer['id']}", {"cover_asset_id": ids["mountain.jpg"]})
    bob_album = bob.ok("POST", "/assets/albums", {"title": "Bob's trip", "asset_ids": [ids["bob/train.jpg"], ids["bob/sunset.jpg"]]})
    state["albums"] = {"summer": summer["id"], "pets": pets["id"], "bob": bob_album["id"]}

    share_album = alice.ok("POST", "/assets/shares", {"album_id": summer["id"], "title": "Summer, shared", "allow_download": True})
    share_sel = alice.ok(
        "POST",
        "/assets/shares",
        {"asset_ids": [ids["city_night.jpg"], ids["forest.jpg"]], "title": "Two photos", "password": random_password(), "allow_download": False, "expires_at": "2099-01-01T00:00:00Z"},
    )
    state["shares"] = {"album": share_album, "selection": {k: v for k, v in share_sel.items() if k != "token"}}
    state["share_token"] = share_album.get("token") or (share_album.get("share") or {}).get("token")

    alice.ok("POST", "/assets/favorite", {"ids": [ids["beach.jpg"], ids["cat.jpg"]], "favorite": True})
    bob.ok("POST", "/assets/favorite", {"ids": [ids["bob/guitar.jpg"]], "favorite": True})
    alice.ok("POST", "/assets/trash", {"ids": [ids["coffee.jpg"]]})

    log("photo edits (date, location, caption)")
    alice.ok(
        "POST",
        "/assets/edits",
        {
            "ids": [ids["horse.jpg"]],
            "date": {"local": "2021-02-03T04:05:06", "offset": "+01:00"},
            "location": {"latitude": PARIS[0], "longitude": PARIS[1]},
            "caption": {"text": "Edited by the drill: a horse in Paris"},
        },
    )
    alice.ok("POST", "/assets/edits", {"ids": [ids["pizza.jpg"]], "caption": {"text": "Caption only"}})

    log("admin task (bucket check)")
    alice.request("POST", "/assets/admin/bucket-check", {})

    with open(a.state, "w") as f:
        json.dump(state, f)
    os.chmod(a.state, 0o600)
    log("seed done")


# ---------------------------------------------------------------------------------------------------- password resets


def cmd_reset_passwords(a):
    """The owner's step after a bucket-only rebuild: a reset link per account (`cyanotype reset-link`), then the old password
    is chosen again through the real endpoint. Proves the accounts are usable again."""
    with open(a.state) as f:
        state = json.load(f)
    c = Client(state["base"])
    for who, u in state["users"].items():
        r = subprocess.run(shlex.split(a.compose) + ["reset-link", "--email", u["email"]], capture_output=True, text=True)
        m = re.search(r"token=([A-Za-z0-9_-]+)", r.stdout)
        if r.returncode != 0 or not m:
            raise SystemExit(f"reset-link for {who} failed: {r.stderr.strip()[-300:]}")
        status, body, _ = c.request("POST", "/auth/reset-password", {"token": m.group(1), "password": u["password"]})
        if status != 204:
            raise SystemExit(f"reset for {who} failed: HTTP {status} {str(body)[:200]}")
        log(f"password of {who} set again through a reset link")


# ---------------------------------------------------------------------------------------------------- settle


def wait_worker(state, quick=False, timeout=1500):
    """Until no job is queued or running, and (unless quick) every visible photo has its embedding."""
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        busy = int(psql_one("SELECT count(*) FROM jobs WHERE state IN ('queued','running')") or 0)
        need_embed = int(
            psql_one(
                "SELECT count(*) FROM assets a WHERE a.purging_at IS NULL AND a.deleted_at IS NULL AND a.primary_id IS NULL "
                "AND NOT EXISTS (SELECT 1 FROM asset_embeddings e WHERE e.asset_id = a.id) "
                "AND EXISTS (SELECT 1 FROM jobs j WHERE j.asset_id = a.id AND j.kind = 'thumbnail' AND j.state = 'done')"
            )
            or 0
        )
        total = int(psql_one("SELECT count(*) FROM assets") or 0)
        want = 0 if quick else need_embed
        cur = (busy, need_embed)
        if cur != last:
            log(f"worker: {busy} job(s) pending, {need_embed} photo(s) still without an embedding ({total} assets)")
            last = cur
        if busy == 0 and want == 0 and total > 0:
            # one more quiet moment: jobs queue follow-ups (burst regroup after embed, pairing after metadata)
            time.sleep(3)
            if int(psql_one("SELECT count(*) FROM jobs WHERE state IN ('queued','running')") or 0) == 0:
                return True
        time.sleep(2)
    log("worker did not finish in time")
    return False


def cmd_settle(a):
    with open(a.state) as f:
        state = json.load(f)
    ok = wait_worker(state, quick=False, timeout=a.timeout)
    failed = psql("SELECT kind, count(*) FROM jobs WHERE state = 'failed' GROUP BY 1")
    if failed:
        log(f"failed jobs: {failed}")
    sys.exit(0 if ok else 1)


# ---------------------------------------------------------------------------------------------------- snapshot

TS = "date_trunc('second', {c})"


def _t(c):
    return f"coalesce(to_char({c} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS'), '')"


# group, name, label SQL, row SQL, FROM/WHERE. Rows are hashed; a row's label is only there to say which one differs.
SECTIONS = [
    ("accounts", "users: identity and role", "email", f"id::text, email, role, {_t('created_at')}", "users"),
    ("accounts", "users: disabled flag", "email", "id::text, (disabled_at IS NOT NULL)::text", "users"),
    ("limits", "users: per-user limits", "email", "id::text, coalesce(quota_bytes::text,''), coalesce(daily_upload_bytes::text,''), coalesce(max_file_bytes::text,'')", "users"),
    ("accounts", "users: password hash", "email", "id::text, md5(password_hash)", "users"),
    ("accounts", "api_keys", "name", "id::text, user_id::text, name, encode(key_hash,'hex'), prefix", "api_keys"),
    ("accounts", "sessions", "user_id::text", "encode(token_hash,'hex'), user_id::text", "sessions"),
    ("photos", "assets: files (id, owner, hash, key, name, type, size, file mtime)", "original_name",
     f"id::text, owner_id::text, encode(sha256,'hex'), original_key, original_name, content_type, size_bytes::text, {_t('file_modified_at')}", "assets"),
    ("photos", "assets: trash state", "original_name", "id::text, (deleted_at IS NOT NULL)::text", "assets"),
    ("photos", "assets: favorites", "original_name", "id::text, favorite::text", "assets"),
    ("photos", "assets: RAW+JPEG pairs", "original_name", "id::text, primary_id::text", "assets WHERE primary_id IS NOT NULL"),
    ("photos", "assets: edits (date, location, caption)", "original_name",
     f"id::text, coalesce(caption,''), date_edited::text, location_edited::text, {_t('taken_at')}, coalesce(taken_offset_edit::text,''), coalesce(latitude::text,''), coalesce(longitude::text,''), {_t('taken_at_exif')}",
     "assets WHERE date_edited OR location_edited OR caption IS NOT NULL"),
    ("derived", "assets: EXIF, size, camera, taken time", "original_name",
     f"id::text, coalesce(width::text,''), coalesce(height::text,''), {_t('taken_at')}, coalesce(latitude::text,''), coalesce(longitude::text,''), coalesce(camera_make,''), coalesce(camera_model,''), coalesce(encode(thumbhash,'hex'),'')", "assets"),
    ("derived", "assets: places (geocode)", "original_name", "id::text, coalesce(place_city,''), coalesce(place_country_code,'')", "assets"),
    ("derived", "assets: bursts", "original_name", "id::text, burst_id::text", "assets WHERE burst_id IS NOT NULL"),
    ("derived", "asset_embeddings", "model", "asset_id::text, model, md5(embedding::text)", "asset_embeddings"),
    ("photos", "albums", "title", f"id::text, owner_id::text, title, coalesce(description,''), coalesce(cover_asset_id::text,''), {_t('created_at')}", "albums"),
    ("photos", "album membership", "album_id::text", "album_id::text, asset_id::text", "album_assets"),
    ("photos", "shares (links and settings)", "coalesce(title,'')",
     f"id::text, owner_id::text, md5(token), kind, coalesce(album_id::text,''), coalesce(title,''), (password_hash IS NOT NULL)::text, allow_download::text, include_originals::text, {_t('expires_at')}", "shares"),
    ("photos", "shares: password hashes", "coalesce(title,'')", "id::text, md5(coalesce(password_hash,''))", "shares"),
    ("photos", "shares: photos of selection links", "share_id::text", "share_id::text, asset_id::text", "share_assets"),
    ("photos", "shares: view counts", "coalesce(title,'')", "id::text, view_count::text", "shares"),
    ("admin", "server defaults (default_limits)", "key", "key, value::text", "server_state WHERE key = 'default_limits'"),
    ("admin", "invites", "coalesce(note,'')", f"id::text, encode(token_hash,'hex'), coalesce(note,''), {_t('expires_at')}, (used_at IS NOT NULL)::text, coalesce(quota_bytes::text,''), coalesce(daily_upload_bytes::text,'')", "invites"),
    ("admin", "password_resets", "user_id::text", "id::text, user_id::text, encode(token_hash,'hex')", "password_resets"),
    ("admin", "admin_tasks", "kind", "kind, state, params::text", "admin_tasks"),
    ("admin", "audit_log", "action", "action, coalesce(actor_email,''), coalesce(target_id,''), detail::text", "audit_log"),
    ("admin", "user_usage counters", "user_id::text", "user_id::text, originals_bytes::text, trash_bytes::text, photo_count::text, raw_count::text, file_count::text, trash_count::text", "user_usage"),
    ("admin", "user_usage: measured derived bytes", "user_id::text", "user_id::text, derived_bytes::text, derived_files::text", "user_usage"),
    ("admin", "upload_log", "owner_id::text", "owner_id::text, bytes::text", "upload_log"),
]

SECTION_NAMES = [(g, n) for g, n, *_ in SECTIONS] + [("photos", "assets: upload time (within 2 s)")]


def table_summary(detail=False):
    """`detail` also keeps each row's text (synthetic drills only: it would hold names and emails)."""
    out = {}
    for group, name, label, row, frm in SECTIONS:
        sql = (
            f"SELECT coalesce(json_agg(json_build_array(l, md5(r){', r' if detail else ''}) ORDER BY l, md5(r)), '[]'::json) FROM "
            f"(SELECT ({label}) AS l, concat_ws('|', {row}) AS r FROM {frm}) t"
        )
        rows = json.loads(psql_one(sql))
        digest = hashlib.sha256(json.dumps(sorted(r[1] for r in rows)).encode()).hexdigest()[:16]
        out[name] = {"group": group, "count": len(rows), "hash": digest, "rows": rows}
    # The upload time: the database stamps it when the upload completes, the object's own metadata when it started, so a
    # rebuild can differ by a moment. Compared with a tolerance (TOLERANCE_SECS), not by hash.
    epochs = {r[0]: float(r[1]) for r in psql("SELECT id::text, extract(epoch FROM created_at) FROM assets")}
    out["assets: upload time (within 2 s)"] = {
        "group": "photos",
        "count": len(epochs),
        "hash": "",
        "rows": [[k, k] for k in sorted(epochs)],
        "epochs": epochs,
    }
    # Jobs and the per-kind picture: informational only (they are history, re-run after a rebuild).
    out["_jobs"] = {k: int(c) for k, c in [(f"{r[0]}:{r[1]}", r[2]) for r in psql("SELECT kind, state, count(*) FROM jobs GROUP BY 1, 2 ORDER BY 1, 2")]}
    out["_bucket"] = None
    return out


# API samples: what a user sees. Presigned URLs and generated times are removed so two runs can be compared.
#   * deleted_at / purge_at: a rebuilt trash clock restarts by design (docs/recovery.md);
#   * updated_at: rows made again by a rebuild are stamped with the rebuild's time.
DROP_KEYS = {"generated_at", "now", "server_time", "expires_in", "deleted_at", "purge_at", "updated_at"}


def canon(v):
    if isinstance(v, dict):
        return {k: canon(x) for k, x in sorted(v.items()) if k not in DROP_KEYS and not k.endswith("_url") and k != "url"}
    if isinstance(v, list):
        return [canon(x) for x in v]
    if isinstance(v, str) and "X-Amz-" in v:
        return "<presigned>"
    return v


def login(base, creds):
    c = Client(base)
    status, body, _ = c.request("POST", "/auth/login", {"email": creds["email"], "password": creds["password"]})
    return c if status in (200, 204) else None


def api_samples(state):
    base = state["base"]
    out = {}
    names = {v: k for k, v in state["assets"].items()}
    out_users = {}
    for who in ("alice", "bob"):
        creds = state["users"][who]
        c = login(base, creds)
        out_users[who] = c
        out[f"{who}: login with the original password"] = {"ok": c is not None}
        if c is None:
            continue
        s, me, _ = c.request("GET", "/auth/me")
        out[f"{who}: /auth/me"] = canon(me) if s == 200 else {"status": s}

    def get(who, key, path):
        c = out_users.get(who)
        if c is None:
            out[key] = {"unavailable": "cannot log in"}
            return None
        s, body, _ = c.request("GET", path)
        out[key] = canon(body) if s == 200 else {"status": s}
        if s == 200 and key.endswith(": map"):
            # A cluster's id is one of its photos' ids, chosen arbitrarily among equals.
            for cl in out[key].get("clusters", []):
                cl.pop("id", None)
        return body if s == 200 else None

    get("alice", "alice: timeline days", "/assets/timeline")
    lst = get("alice", "alice: library list", "/assets?limit=100")
    get("alice", "alice: favorites list", "/assets?limit=100&favorite=true")
    get("alice", "alice: trash", "/assets/trash")
    albums = get("alice", "alice: albums", "/assets/albums")
    for item in (albums or {}).get("items", albums if isinstance(albums, list) else []):
        get("alice", f"alice: album contents: {item.get('title')}", f"/assets/albums/{item['id']}/assets")
    get("alice", "alice: search 'a photo of a dog'", "/assets/search?q=" + urllib.parse.quote("a photo of a dog"))
    get("alice", "alice: search by place 'Lisbon'", "/assets/search?place=Lisbon")
    get("alice", "alice: map", "/assets/map?zoom=3")
    get("alice", "alice: usage", "/assets/usage")
    get("alice", "alice: shares", "/assets/shares")
    for name in ("horse.jpg", "pizza.jpg", "beach.jpg", "IMG_0001.JPG", "burst_1.jpg"):
        aid = state["assets"].get(name)
        if aid:
            get("alice", f"alice: photo details: {name}", f"/assets/{aid}/details")
    get("bob", "bob: library list", "/assets?limit=100")
    get("bob", "bob: albums", "/assets/albums")
    get("bob", "bob: usage", "/assets/usage")
    # the public share page: no login at all
    pub = Client(base)
    token = state.get("share_token")
    if token:
        for key, path in (("share page (info)", f"/assets/public/s/{token}"), ("share page (items)", f"/assets/public/s/{token}/items")):
            s, body, _ = pub.request("GET", path)
            out[key] = canon(body) if s == 200 else {"status": s}
    # API keys keep working?
    for who in ("alice", "bob"):
        k = state["users"][who].get("api_key")
        if k:
            c = Client(base)
            c.bearer = k
            s, body, _ = c.request("GET", "/assets/count")
            out[f"{who}: old API key still works"] = {"ok": s == 200, "count": canon(body) if s == 200 else None}
    # The session cookie from before the disaster
    sess = state.get("session_cookies")
    return out


def cmd_snapshot(a):
    stamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    if a.no_api:
        snap = {"tables": table_summary(), "api": {}, "assets": {}, "taken_at": stamp}
        with open(a.out, "w") as f:
            json.dump(snap, f, indent=1, sort_keys=True)
        log("snapshot written (tables only)")
        return
    with open(a.state) as f:
        state = json.load(f)
    snap = {"tables": table_summary(detail=True), "api": api_samples(state), "assets": state["assets"], "taken_at": stamp}
    if a.bucket and os.path.exists(a.bucket):
        snap["bucket"] = json.loads(open(a.bucket).read())
    with open(a.out, "w") as f:
        json.dump(snap, f, indent=1, sort_keys=True)
    log(f"snapshot written ({sum(t['count'] for k, t in snap['tables'].items() if not k.startswith('_'))} rows over {len(SECTIONS)} summaries)")


# ---------------------------------------------------------------------------------------------------- report

OK, PART, LOST, NA = "✅ restored", "⚠️ partially", "❌ lost", "➖ n/a"


def judge(before, after):
    """(verdict, detail) comparing one table summary across a recovery."""
    nb, na = before["count"], after["count"]
    if nb == 0 and na == 0:
        return NA, "nothing seeded"
    if "epochs" in before:
        ea = after.get("epochs", {})
        same = sum(1 for k, v in before["epochs"].items() if k in ea and abs(ea[k] - v) <= 2.0)
        if same == nb:
            return OK, f"{nb}/{nb} within 2 s"
        return (PART if same else LOST), f"{same} of {nb} within 2 s"
    b = {tuple(r[:2]) for r in before["rows"]}
    af = {tuple(r[:2]) for r in after["rows"]}
    same = len(b & af)
    if b == af:
        return OK, f"{nb}/{nb} rows identical"
    if same == 0:
        return LOST, f"0 of {nb} rows survive" + (f" ({na} different rows present)" if na else "")
    return PART, f"{same} of {nb} rows identical" + (f", {na - same} different/extra" if na - same else "")


def judge_api(before, after):
    if before == after:
        return OK, "same response"
    if isinstance(after, dict) and ("unavailable" in after or after.get("status") in (401, 403, 404)):
        return LOST, "not available"
    return PART, "response differs"


# What a bucket-only rebuild deliberately does not bring back (docs/recovery.md), shown next to the verdict.
BY_DESIGN_B = {
    "users: password hash": "no password is ever stored in the bucket; the owner sets a new one with a reset link",
    "api_keys": "credentials stay out of the bucket; make new keys",
    "sessions": "everyone signs in again",
    "shares: password hashes": "the link comes back closed; its owner sets a new password",
    "shares: view counts": "statistics",
    "invites": "ephemeral; issue new ones",
    "password_resets": "ephemeral",
    "admin_tasks": "ephemeral",
    "audit_log": "an operational record kept in the dump, not in the bucket",
    "upload_log": "usage history; the 24 hour cap window starts empty",
    "user_usage: measured derived bytes": "re-measured by the worker over time",
}

FEATURE_ORDER = [
    ("accounts", "Accounts"),
    ("limits", "Limits"),
    ("photos", "Photos and what is done with them"),
    ("derived", "Derived data (regenerated by the worker)"),
    ("admin", "Admin and housekeeping"),
]


def cmd_report(a):
    snaps = {k: json.load(open(getattr(a, k))) for k in ("before", "path_a", "path_b")}
    timings = []
    if os.path.exists(a.timings):
        for line in open(a.timings):
            step, secs = line.rstrip("\n").split("\t")
            timings.append((step, float(secs)))
    lines = []
    w = lines.append
    w(f"# Restore drill report{' (' + a.title + ')' if a.title else ''}")
    w("")
    w("> **Synthetic data.** Generated by `scripts/restore-drill.sh` against a throwaway stack with test photos and made-up")
    w("> accounts (`*@drill.invalid`). It contains nothing from a real library, server or bucket.")
    w("")
    w(f"- Commit: `{a.commit}`   - Mode: `{a.mode}`   - Files seeded: {len(snaps['before']['assets'])}   - Run: {snaps['before']['taken_at']}")
    w("")
    w("Path A: restore the nightly Postgres dump, migrate, start. Path B: lose the database, `cyanotype migrate` + `cyanotype reindex --apply` from the bucket alone, start.")
    w("A row is judged by comparing every row of the table (hashed important columns) before and after; the worker is given time to finish its queue first.")
    w("")
    tb, ta, tc = snaps["before"]["tables"], snaps["path_a"]["tables"], snaps["path_b"]["tables"]
    w("## Tables and features")
    w("")
    problems_a, problems_b = [], []
    for group, title in FEATURE_ORDER:
        w(f"### {title}")
        w("")
        w("| Table / feature | Seeded rows | Path A (dump) | Path B (bucket only) |")
        w("|---|---:|---|---|")
        for g, name in SECTION_NAMES:
            if g != group:
                continue
            va, da = judge(tb[name], ta[name])
            vb, db_ = judge(tb[name], tc[name])
            if va == NA and vb == NA:
                continue
            if vb in (LOST, PART) and name in BY_DESIGN_B:
                db_ += f" *(by design: {BY_DESIGN_B[name]})*"
            w(f"| {name} | {tb[name]['count']} | {va}: {da} | {vb}: {db_} |")
            if va in (LOST, PART):
                problems_a.append(name)
            if vb in (LOST, PART):
                problems_b.append(name)
        w("")
    w("## What a user sees (API responses)")
    w("")
    w("| Sample | Path A | Path B |")
    w("|---|---|---|")
    for key in snaps["before"]["api"]:
        va, da = judge_api(snaps["before"]["api"][key], snaps["path_a"]["api"].get(key, {"unavailable": 1}))
        vb, db_ = judge_api(snaps["before"]["api"][key], snaps["path_b"]["api"].get(key, {"unavailable": 1}))
        if vb in (LOST, PART) and key.endswith("old API key still works"):
            db_ += " *(by design: API keys are not in the bucket; make new ones)*"
        if vb in (LOST, PART) and key.endswith(": usage"):
            db_ += " *(by design: the upload history starts empty; the totals are recomputed)*"
        w(f"| {key} | {va}: {da} | {vb}: {db_} |")
    w("")
    bk = {k: snaps[k].get("bucket", {}).get("summary") for k in ("before", "path_a", "path_b")}
    if all(bk.values()):
        w("## The bucket (what recovery must never change)")
        w("")
        w("| Prefix | Before | After A | After B |")
        w("|---|---|---|---|")
        for p in bk["before"]:
            cells = []
            for k in ("before", "path_a", "path_b"):
                v = bk[k][p]
                cells.append(f"{v['objects']} objects" + ("" if k == "before" or v["hash"] == bk["before"][p]["hash"] else " (changed)"))
            w(f"| `{p}` | " + " | ".join(cells) + " |")
        w("")
        w("`originals/`, `edits/` and `backups/` must be identical; `derived/` may gain objects (a rebuilt photo keeps its old id only when a manifest knew it) and `manifests/` is rewritten by the daily job.")
        w("")
    for title, path in (("Path A: `reindex` dry run after the dump restore", a.a_reindex), ("Path B: `reindex` dry run on the empty database", a.b_dry), ("Path B: `reindex --apply`", a.b_apply)):
        if path and os.path.exists(path):
            try:
                rep = json.loads(open(path).read())
            except ValueError:
                continue
            keep = [
                "scanned_objects", "present", "orphans", "orphans_recreated", "orphans_unknown_user", "orphans_reassigned",
                "missing_originals", "users_restored", "users_present", "users_conflict", "albums_restored",
                "album_items_restored", "pairs_restored", "favorites_restored", "trashed_restored", "shares_restored",
                "edits_objects", "edits_restored", "manifest_users", "manifest_files", "errors",
            ]
            w(f"### {title}")
            w("")
            w("`" + ", ".join(f"{k}={rep[k]}" for k in keep if k in rep) + "`")
            w("")
    w("## Timings")
    w("")
    w("| Step | Seconds |")
    w("|---|---:|")
    for step, secs in timings:
        w(f"| {step} | {secs:.1f} |")
    w("")
    n = len(snaps["before"]["assets"])
    w(f"Photos in the drill library: {n} files (a few KB each). Per-photo costs below are dominated by fixed start-up, so they say little about")
    w("2,000 photos / 25 GB; see `docs/recovery.md` for the extrapolation and what scales with count versus bytes.")
    w("")
    w("## Lost or changed")
    w("")
    w(f"- Path A: {', '.join(problems_a) if problems_a else 'nothing'}")
    w(f"- Path B: {', '.join(problems_b) if problems_b else 'nothing'}")
    w("")
    text = "\n".join(lines) + "\n"
    with open(a.out, "w") as f:
        f.write(text)
    print(text)
    with open(a.out + ".problems.json", "w") as f:
        json.dump({"a": problems_a, "b": problems_b}, f)


# ---------------------------------------------------------------------------------------------------- bucket tools


def _s3_env(prefix):
    """(endpoint, bucket, access, secret, region) from the environment. `prefix` is DRILL_S3 (real mode) or DRILL_LOCAL_S3."""
    if prefix == "DRILL_S3":
        return (
            os.environ.get("DRILL_PROBE_ENDPOINT") or os.environ["DRILL_S3_ENDPOINT"],
            os.environ["DRILL_S3_BUCKET"],
            os.environ["DRILL_S3_ACCESS_KEY_ID"],
            os.environ["DRILL_S3_SECRET_ACCESS_KEY"],
            os.environ.get("DRILL_S3_REGION", "us-east-1"),
        )
    return (
        os.environ["DRILL_LOCAL_S3_URL"],
        "cyanotype",
        os.environ["DRILL_LOCAL_S3_ACCESS_KEY"],
        os.environ["DRILL_LOCAL_S3_SECRET_KEY"],
        "us-east-1",
    )


def cmd_probe(a):
    """Refuse (exit 3) if the key can write: a read-only key's PUT must be denied. Exit 4 if it cannot even read."""
    endpoint, bucket, access, secret, region = _s3_env(a.env)
    status, key = s3.probe_write(endpoint, bucket, access, secret, region)
    if 200 <= status < 300:
        removed = s3.delete_key(endpoint, bucket, access, secret, key, region)
        print(
            f"REFUSING TO RUN: the key could WRITE (PUT drill-probe/... answered HTTP {status}). It is not read-only. "
            f"A tiny object was written under drill-probe/ (cleanup DELETE answered HTTP {removed}; on a versioned bucket "
            "a delete marker and the old version remain: remove them by hand). "
            "Create a read-only key (docs/recovery.md, 'Real-data drill') and run again.",
            file=sys.stderr,
        )
        sys.exit(3)
    if status in (401, 403):
        print(f"write probe denied (HTTP {status}): the key is read-only", file=sys.stderr)
    else:
        print(f"write probe answered HTTP {status}, which is neither success nor a clear denial; refusing to continue", file=sys.stderr)
        sys.exit(3)
    # It must be able to read, or the drill cannot work.
    s, _body = s3.request(endpoint, bucket, access, secret, query={"list-type": "2", "max-keys": "1"}, region=region)
    if s != 200:
        print(f"the key cannot list the bucket (HTTP {s}); it needs read and list access", file=sys.stderr)
        sys.exit(4)
    print("read access confirmed", file=sys.stderr)


def cmd_bucket(a):
    endpoint, bucket, access, secret, region = _s3_env(a.env)
    keys = {}
    for prefix in ("originals/", "derived/", "manifests/", "edits/", "backups/"):
        keys[prefix] = sorted(s3.list_keys(endpoint, bucket, access, secret, prefix, region))
    summary = {p: {"objects": len(k), "hash": hashlib.sha256("\n".join(k).encode()).hexdigest()[:16]} for p, k in keys.items()}
    with open(a.out, "w") as f:
        json.dump({"summary": summary, "keys": keys if a.keys else None}, f, indent=1, sort_keys=True)
    log("bucket: " + ", ".join(f"{p} {v['objects']}" for p, v in summary.items()))


def cmd_report_real(a):
    """Real-data mode: counts and verdicts only (no names, emails or titles), so the report holds nothing identifying."""
    dump, rebuild = json.load(open(a.dump)), json.load(open(a.rebuild))
    dry, apply_ = json.loads(open(a.dry).read()), json.loads(open(a.apply).read())
    timings = [line.rstrip("\n").split("\t") for line in open(a.timings)]
    lines = []
    w = lines.append
    w("# Real-data restore drill (read-only)")
    w("")
    w("> Counts and verdicts only: no names, emails or titles. It still describes your library: **do not commit or publish it.**")
    w("")
    w(f"- Commit: `{a.commit}`   - Dump restored: the newest in the bucket   - Worker not run: derived data is not compared")
    w("")
    w("Left column: the restored dump (path A: by definition what it holds). Right: what `reindex --apply` rebuilt from the bucket alone into a scratch database (path B).")
    w("")
    w("| Table / feature | In the dump | Path B (bucket only) |")
    w("|---|---:|---|")
    skip_derived = {g for g in ("derived",)}
    for group, name in SECTION_NAMES:
        if group in skip_derived:
            continue
        v, det = judge(dump["tables"][name], rebuild["tables"][name])
        if v == NA:
            continue
        w(f"| {name} | {dump['tables'][name]['count']} | {v}: {det} |")
    w("")
    w("Reading the table: the worker did not run, so a row that holds a value the worker computes from the file (the EXIF time kept beside an")
    w("edit, measured derived bytes) reads as partial or lost here; the same rows are fully restored once the worker has run (synthetic drill).")
    w("Credentials, invites, the audit log and upload history are not in the bucket by design (docs/recovery.md).")
    w("")
    w("## `reindex` dry run against the restored dump (what the dump is missing from the bucket)")
    w("")
    for k in ("scanned_objects", "present", "orphans", "orphans_recreated", "orphans_unknown_user", "missing_originals", "thumbs_missing", "manifest_users", "manifest_files", "edits_objects", "edits_restored", "errors"):
        if k in dry:
            w(f"- {k}: {dry[k]}")
    w("")
    w("## What `reindex --apply` did in the scratch database")
    w("")
    for k in ("users_restored", "users_conflict", "orphans_recreated", "orphans_unknown_user", "albums_restored", "album_items_restored", "shares_restored", "favorites_restored", "trashed_restored", "pairs_restored", "edits_restored", "errors"):
        if k in apply_:
            w(f"- {k}: {apply_[k]}")
    w("")
    w("## Timings")
    w("")
    w("| Step | Seconds |")
    w("|---|---:|")
    for step, secs in timings:
        w(f"| {step} | {float(secs):.1f} |")
    w("")
    text = "\n".join(lines) + "\n"
    with open(a.out, "w") as f:
        f.write(text)
    print(text)


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("seed")
    s.add_argument("--state", required=True)
    s.add_argument("--cache", default=os.path.join(REPO, "models", "eval", "raw"))
    s.set_defaults(fn=cmd_seed)
    s = sub.add_parser("settle")
    s.add_argument("--state", required=True)
    s.add_argument("--timeout", type=int, default=1500)
    s.set_defaults(fn=cmd_settle)
    s = sub.add_parser("snapshot")
    s.add_argument("--state")
    s.add_argument("--out", required=True)
    s.add_argument("--bucket")
    s.add_argument("--no-api", action="store_true")
    s.set_defaults(fn=cmd_snapshot)
    s = sub.add_parser("reset-passwords")
    s.add_argument("--state", required=True)
    s.add_argument("--compose", required=True, help="command that runs the cyanotype CLI in the drill stack")
    s.set_defaults(fn=cmd_reset_passwords)
    s = sub.add_parser("probe")
    s.add_argument("--env", choices=["DRILL_S3", "DRILL_LOCAL_S3"], default="DRILL_S3")
    s.set_defaults(fn=cmd_probe)
    s = sub.add_parser("bucket")
    s.add_argument("--env", choices=["DRILL_S3", "DRILL_LOCAL_S3"], default="DRILL_LOCAL_S3")
    s.add_argument("--out", required=True)
    s.add_argument("--keys", action="store_true")
    s.set_defaults(fn=cmd_bucket)
    s = sub.add_parser("report-real")
    for k in ("dump", "rebuild", "dry", "apply", "timings", "out"):
        s.add_argument("--" + k, required=True)
    s.add_argument("--commit", default="unknown")
    s.set_defaults(fn=cmd_report_real)
    s = sub.add_parser("report")
    for k in ("before", "path_a", "path_b", "timings", "out"):
        s.add_argument("--" + k.replace("_", "-"), dest=k, required=True)
    s.add_argument("--commit", default="unknown")
    s.add_argument("--mode", default="synthetic")
    s.add_argument("--title", default="")
    for k in ("a-reindex", "b-dry", "b-apply"):
        s.add_argument("--" + k)
    s.set_defaults(fn=cmd_report)
    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
