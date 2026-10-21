"""Synthetic photo files for the restore drill: real (public-domain) test JPEGs with EXIF written onto them.

Only the standard library. The pictures come from the repo's own test fixtures (crates/cyanotype-clip/tests/fixtures,
CC0 / public domain, see SOURCES.json there) and optionally the cached evaluation set; EXIF (camera, capture time, GPS)
is written here, so the drill knows exactly what the worker should find. Nothing in them is personal data.
"""
import os
import struct

BE = ">"


def _rat(n, d):
    return struct.pack(BE + "II", n, d)


def _ascii(s):
    return s.encode() + b"\0"


class _Entry:
    def __init__(self, tag, typ, count, data):
        self.tag, self.typ, self.count, self.data = tag, typ, count, data


def _entry_ascii(tag, s):
    b = _ascii(s)
    return _Entry(tag, 2, len(b), b)


def _entry_long(tag, v):
    return _Entry(tag, 4, 1, struct.pack(BE + "I", v))


def _entry_rationals(tag, pairs):
    return _Entry(tag, 5, len(pairs), b"".join(_rat(n, d) for n, d in pairs))


def _ifd_size(entries):
    inline = 2 + 12 * len(entries) + 4
    return inline + sum(len(e.data) + (len(e.data) & 1) for e in entries if len(e.data) > 4)


def _ifd(entries, base):
    entries = sorted(entries, key=lambda e: e.tag)
    head = struct.pack(BE + "H", len(entries))
    data_off = base + 2 + 12 * len(entries) + 4
    body = b""
    tail = b""
    for e in entries:
        if len(e.data) <= 4:
            val = e.data.ljust(4, b"\0")
        else:
            val = struct.pack(BE + "I", data_off + len(tail))
            tail += e.data + (b"\0" if len(e.data) & 1 else b"")
        body += struct.pack(BE + "HHI", e.tag, e.typ, e.count) + val
    return head + body + struct.pack(BE + "I", 0) + tail


def _dms(value):
    value = abs(value)
    d = int(value)
    m = int((value - d) * 60)
    s = round(((value - d) * 60 - m) * 60 * 10000)
    return [(d, 1), (m, 1), (s, 10000)]


def exif_app1(make, model, taken, gps=None):
    """APP1 segment with Make/Model, DateTimeOriginal (`YYYY:MM:DD HH:MM:SS`) and optional GPS (lat, lon)."""
    ifd0 = [_entry_ascii(0x010F, make), _entry_ascii(0x0110, model), _entry_long(0x8769, 0)]
    if gps:
        ifd0.append(_entry_long(0x8825, 0))
    exif = [_entry_ascii(0x9003, taken), _entry_ascii(0x9004, taken)]
    g = None
    if gps:
        lat, lon = gps
        g = [
            _entry_ascii(0x0001, "N" if lat >= 0 else "S"),
            _entry_rationals(0x0002, _dms(lat)),
            _entry_ascii(0x0003, "E" if lon >= 0 else "W"),
            _entry_rationals(0x0004, _dms(lon)),
        ]
    off_exif = 8 + _ifd_size(ifd0)
    off_gps = off_exif + _ifd_size(exif)
    ifd0 = [e if e.tag not in (0x8769, 0x8825) else _entry_long(e.tag, off_exif if e.tag == 0x8769 else off_gps) for e in ifd0]
    tiff = b"MM\x00\x2a" + struct.pack(BE + "I", 8) + _ifd(ifd0, 8) + _ifd(exif, off_exif)
    if g:
        tiff += _ifd(g, off_gps)
    payload = b"Exif\0\0" + tiff
    return b"\xff\xe1" + struct.pack(BE + "H", len(payload) + 2) + payload


def with_exif(jpeg, app1, tail=b""):
    """The JPEG with its own EXIF replaced by `app1`; `tail` is appended after the end marker (a unique hash, same picture)."""
    assert jpeg[:2] == b"\xff\xd8"
    out = [b"\xff\xd8", app1]
    i = 2
    while i < len(jpeg):
        if jpeg[i] != 0xFF:
            break
        marker = jpeg[i + 1]
        if marker == 0xDA:  # start of scan: the rest is image data
            break
        (length,) = struct.unpack(BE + "H", jpeg[i + 2 : i + 4])
        seg = jpeg[i : i + 2 + length]
        i += 2 + length
        if marker == 0xE1 and seg[4:10] == b"Exif\0\0":
            continue
        out.append(seg)
    out.append(jpeg[i:])
    return b"".join(out) + tail


def tiff_exif(path):
    """(make, model, 'YYYY:MM:DD HH:MM:SS') read from a TIFF-based RAW file (NEF, ARW, DNG, ...), or None."""
    with open(path, "rb") as f:
        b = f.read(4 * 1024 * 1024)
    if b[:2] not in (b"II", b"MM"):
        return None
    e = "<" if b[:2] == b"II" else ">"

    def u(fmt, off):
        return struct.unpack_from(e + fmt, b, off)[0]

    def entries(off):
        n = u("H", off)
        for k in range(n):
            o = off + 2 + 12 * k
            yield u("H", o), u("H", o + 2), u("I", o + 4), o + 8

    def text(cnt, vo):
        o = u("I", vo) if cnt > 4 else vo
        return b[o : o + cnt].split(b"\0")[0].decode("latin1").strip()

    try:
        make = model = taken = None
        exif_ifd = None
        for tag, _t, cnt, vo in entries(u("I", 4)):
            if tag == 0x010F:
                make = text(cnt, vo)
            elif tag == 0x0110:
                model = text(cnt, vo)
            elif tag == 0x8769:
                exif_ifd = u("I", vo)
        if exif_ifd:
            for tag, _t, cnt, vo in entries(exif_ifd):
                if tag == 0x9003:
                    taken = text(cnt, vo)
        if make and model and taken:
            return make, model, taken
    except struct.error:
        return None
    return None


def read(path):
    with open(path, "rb") as f:
        return f.read()


def fixture_dir(repo):
    return os.path.join(repo, "crates", "cyanotype-clip", "tests", "fixtures", "images")
