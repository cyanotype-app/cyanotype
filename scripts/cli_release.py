#!/usr/bin/env python3
"""Decisions behind the automatic CLI release tags (cli-vX.Y.Z), docs/releases.md.

  cli_release.py changed BASE HEAD     exit 0 and print "true" if the CLI changed between two commits, else "false"
  cli_release.py bump LABELS BODY      print major|minor|patch|none (LABELS: comma separated; BODY: file or '-' for stdin)
  cli_release.py next [BUMP]           print the next version after the highest cli-v tag (default bump: patch)
  cli_release.py current REV           print the version of the newest cli-v tag reachable from REV (empty if none)

"The CLI changed" means: a file changed under apps/cli or under any workspace crate the CLI depends on
(`cargo metadata`, normal and build dependencies; Cargo.lock does not count by itself), or the resolved dependency set of
the CLI (names, versions, sources, enabled features) differs between the two commits, which is how a Cargo.lock or
Cargo.toml change that matters to the CLI shows up. Anything else (docs, other apps, tests of other crates) is not a CLI
change. Python 3 standard library only.
"""
import json
import os
import re
import subprocess
import sys
import tempfile

PACKAGE = "cyanotype-cli"
TAG_PREFIX = "cli-v"
TAG_RE = re.compile(r"^cli-v(\d+)\.(\d+)\.(\d+)$")
BUMPS = ("major", "minor", "patch", "none")


def run(*cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout


# ---- what the CLI depends on ------------------------------------------------------------------------------------


def closure(metadata, root_dir):
    """(local package directories relative to root_dir, sorted fingerprint of the whole resolved closure)."""
    pkgs = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    start = next(i for i, p in pkgs.items() if p["name"] == PACKAGE)
    seen, stack = set(), [start]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            # Dev-dependencies do not end up in the program; "normal" is null, "build" is a build script's.
            if any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    dirs, fingerprint = [], []
    for i in sorted(seen):
        p = pkgs[i]
        fingerprint.append([p["name"], p["version"], p["source"], sorted(nodes[i].get("features", []))])
        if p["source"] is None:
            rel = os.path.relpath(os.path.dirname(p["manifest_path"]), root_dir)
            dirs.append(rel.replace(os.sep, "/"))
    return sorted(dirs), sorted(fingerprint, key=json.dumps)


def metadata_at(directory):
    # Features are resolved the way the downloadable CLI is built (no default features).
    env = dict(os.environ, RUSTUP_TOOLCHAIN=os.environ.get("RUSTUP_TOOLCHAIN", "stable"))
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
        cwd=directory, check=True, capture_output=True, text=True, env=env,
    ).stdout
    return json.loads(out)


def under(path, dirs):
    return any(path == d or path.startswith(d + "/") for d in dirs)


def changed(base, head, repo="."):
    repo = os.path.abspath(repo)
    files = run("git", "diff", "--name-only", base, head, cwd=repo).splitlines()
    with tempfile.TemporaryDirectory() as tmp:
        trees = {}
        for name, rev in (("base", base), ("head", head)):
            path = os.path.join(tmp, name)
            run("git", "worktree", "add", "--detach", path, rev, cwd=repo)
            trees[name] = path
        try:
            results = {}
            for name, path in trees.items():
                try:
                    results[name] = closure(metadata_at(path), path)
                except subprocess.CalledProcessError as e:
                    # An older commit that cannot be resolved offline: be safe and call it a change.
                    sys.stderr.write(f"cargo metadata failed at {name}: {e.stderr[-300:]}\n")
                    return True
        finally:
            for path in trees.values():
                subprocess.run(["git", "worktree", "remove", "--force", path], cwd=repo, capture_output=True)
    (base_dirs, base_fp), (head_dirs, head_fp) = results["base"], results["head"]
    dirs = sorted(set(base_dirs) | set(head_dirs))
    return decide(files, dirs, base_fp, head_fp)


def decide(files, dirs, base_fp, head_fp):
    """Pure core of `changed`: touched files in the CLI's own crates, or a different resolved closure."""
    if any(under(f, dirs) for f in files):
        return True
    return base_fp != head_fp


# ---- bump -------------------------------------------------------------------------------------------------------


def bump_from(labels, body):
    """`release:<level>` labels win; else a `Release: <level>` line in the PR body; else patch.
    Several labels: the biggest bump wins, except that `release:none` (an explicit skip) beats everything."""
    found = []
    for label in labels:
        m = re.fullmatch(r"\s*release:(major|minor|patch|none)\s*", label, re.I)
        if m:
            found.append(m.group(1).lower())
    if not found:
        for line in (body or "").splitlines():
            m = re.fullmatch(r"\s*release:\s*(major|minor|patch|none)\s*", line, re.I)
            if m:
                found.append(m.group(1).lower())
                break
    if not found:
        return "patch"
    if "none" in found:
        return "none"
    return min(found, key=BUMPS.index)


def bump_version(version, bump):
    major, minor, patch = version
    if bump == "major":
        return (major + 1, 0, 0)
    if bump == "minor":
        return (major, minor + 1, 0)
    return (major, minor, patch + 1)


def parse_tags(tags):
    out = []
    for t in tags:
        m = TAG_RE.match(t.strip())
        if m:
            out.append(tuple(int(x) for x in m.groups()))
    return sorted(out)


def next_version(tags, bump="patch"):
    """The next version; with no tags yet the base is 0.0.0, so the first release is 0.0.1 (patch), 0.1.0 or 1.0.0."""
    existing = parse_tags(tags)
    return bump_version(existing[-1] if existing else (0, 0, 0), bump)


def current_version(tags):
    existing = parse_tags(tags)
    return ".".join(map(str, existing[-1])) if existing else ""


def main(argv):
    cmd = argv[1] if len(argv) > 1 else ""
    if cmd == "changed" and len(argv) == 4:
        print("true" if changed(argv[2], argv[3]) else "false")
    elif cmd == "bump" and len(argv) == 4:
        body = sys.stdin.read() if argv[3] == "-" else (open(argv[3]).read() if os.path.exists(argv[3]) else "")
        print(bump_from([x for x in argv[2].split(",") if x], body))
    elif cmd == "next" and len(argv) in (2, 3):
        tags = run("git", "tag", "--list", TAG_PREFIX + "*").splitlines()
        print(".".join(map(str, next_version(tags, argv[2] if len(argv) == 3 else "patch"))))
    elif cmd == "current" and len(argv) == 3:
        tags = run("git", "tag", "--merged", argv[2], "--list", TAG_PREFIX + "*").splitlines()
        print(current_version(tags))
    else:
        sys.stderr.write(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
