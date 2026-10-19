"""A tiny SigV4 S3 client (standard library only) for the restore drill: list a bucket, and probe whether a key can write.

Path-style requests (`endpoint/bucket/key`), like the services use. Credentials come from the caller; nothing is printed.
"""
import datetime
import hashlib
import hmac
import html
import re
import urllib.error
import urllib.parse
import urllib.request


def _sign(key, msg):
    return hmac.new(key, msg.encode(), hashlib.sha256).digest()


def request(endpoint, bucket, access, secret, method="GET", key="", query=None, body=b"", region="us-east-1", headers=None):
    """Returns (status, body bytes). HTTP errors are returned, not raised."""
    endpoint = endpoint.rstrip("/")
    host = urllib.parse.urlparse(endpoint).netloc
    path = "/" + bucket + ("/" + urllib.parse.quote(key, safe="/~") if key else "")
    q = "&".join(
        f"{urllib.parse.quote(k, safe='')}={urllib.parse.quote(v, safe='')}" for k, v in sorted((query or {}).items())
    )
    now = datetime.datetime.now(datetime.timezone.utc)
    amz, day = now.strftime("%Y%m%dT%H%M%SZ"), now.strftime("%Y%m%d")
    payload_hash = hashlib.sha256(body).hexdigest()
    hdrs = {"host": host, "x-amz-content-sha256": payload_hash, "x-amz-date": amz}
    for k, v in (headers or {}).items():
        hdrs[k.lower()] = v
    names = sorted(hdrs)
    canon = (
        f"{method}\n{path}\n{q}\n" + "".join(f"{n}:{hdrs[n].strip()}\n" for n in names) + f"\n{';'.join(names)}\n{payload_hash}"
    )
    scope = f"{day}/{region}/s3/aws4_request"
    sts = f"AWS4-HMAC-SHA256\n{amz}\n{scope}\n{hashlib.sha256(canon.encode()).hexdigest()}"
    k = ("AWS4" + secret).encode()
    for part in (day, region, "s3", "aws4_request"):
        k = _sign(k, part)
    sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
    hdrs["authorization"] = (
        f"AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={';'.join(names)}, Signature={sig}"
    )
    del hdrs["host"]
    req = urllib.request.Request(endpoint + path + ("?" + q if q else ""), data=body if method in ("PUT", "POST") else None, method=method, headers=hdrs)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def list_keys(endpoint, bucket, access, secret, prefix="", region="us-east-1"):
    """Every current key under `prefix` (ListObjectsV2, paginated)."""
    keys, token = [], None
    while True:
        q = {"list-type": "2", "prefix": prefix, "max-keys": "1000"}
        if token:
            q["continuation-token"] = token
        status, body = request(endpoint, bucket, access, secret, query=q, region=region)
        if status != 200:
            raise RuntimeError(f"list {prefix!r} failed: HTTP {status} {body[:200]!r}")
        text = body.decode()
        keys += [html.unescape(k) for k in re.findall(r"<Key>([^<]*)</Key>", text)]
        m = re.search(r"<NextContinuationToken>([^<]*)</NextContinuationToken>", text)
        if "<IsTruncated>true</IsTruncated>" in text and m:
            token = m.group(1)
        else:
            return keys


def probe_write(endpoint, bucket, access, secret, region="us-east-1"):
    """Try to PUT a tiny object under `drill-probe/`. Returns (status, key). The write is expected to be DENIED (4xx)."""
    import secrets

    key = f"drill-probe/{secrets.token_hex(8)}"
    status, _ = request(endpoint, bucket, access, secret, method="PUT", key=key, body=b"restore-drill write probe", region=region, headers={"content-type": "text/plain"})
    return status, key


def delete_key(endpoint, bucket, access, secret, key, region="us-east-1"):
    return request(endpoint, bucket, access, secret, method="DELETE", key=key, region=region)[0]
