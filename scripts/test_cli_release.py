#!/usr/bin/env python3
"""Tests for scripts/cli_release.py:  python3 -I -m unittest discover -s scripts -p 'test_cli_release.py'"""
import contextlib
import io
import os
import shutil
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cli_release as r  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class Bump(unittest.TestCase):
    def test_default_is_patch(self):
        self.assertEqual(r.bump_from([], ""), "patch")
        self.assertEqual(r.bump_from(["bug", "docs"], "Some text\nRelease soon"), "patch")

    def test_labels(self):
        for level in ("major", "minor", "patch", "none"):
            self.assertEqual(r.bump_from([f"release:{level}"], ""), level)
        self.assertEqual(r.bump_from(["Release:Minor"], ""), "minor")

    def test_body_line(self):
        self.assertEqual(r.bump_from([], "Fixes things.\n\nRelease: minor\n"), "minor")
        self.assertEqual(r.bump_from([], "release:  NONE"), "none")
        self.assertEqual(r.bump_from([], "text Release: major in a sentence"), "patch")

    def test_label_beats_body_and_none_beats_all(self):
        self.assertEqual(r.bump_from(["release:patch"], "Release: major"), "patch")
        self.assertEqual(r.bump_from(["release:minor", "release:major"], ""), "major")
        self.assertEqual(r.bump_from(["release:minor", "release:none"], ""), "none")


class Versions(unittest.TestCase):
    def test_next(self):
        tags = ["cli-v0.1.9", "cli-v0.1.10", "cli-v0.2.0-rc1", "v9.9.9", "cli-v0.1.2"]
        self.assertEqual(r.next_version(tags), (0, 1, 11))
        self.assertEqual(r.next_version(tags, "minor"), (0, 2, 0))
        self.assertEqual(r.next_version(tags, "major"), (1, 0, 0))
        self.assertEqual(r.next_version([]), (0, 0, 1))
        self.assertEqual(r.next_version([], "minor"), (0, 1, 0))

    def test_current(self):
        self.assertEqual(r.current_version(["cli-v0.1.9", "cli-v0.1.10", "v1.0.0"]), "0.1.10")
        self.assertEqual(r.current_version(["v1.0.0"]), "")


def meta(pkgs, edges):
    """Cargo-metadata-shaped data: pkgs {name: source}, edges {name: [(dep, kind)]}."""
    ids = {n: f"{n} 1.0.0" for n in pkgs}
    return {
        "packages": [
            {"id": ids[n], "name": n, "version": "1.0.0", "source": s, "manifest_path": f"/w/{n}/Cargo.toml"}
            for n, s in pkgs.items()
        ],
        "resolve": {
            "nodes": [
                {
                    "id": ids[n],
                    "features": [],
                    "deps": [{"pkg": ids[d], "dep_kinds": [{"kind": k}]} for d, k in edges.get(n, [])],
                }
                for n in pkgs
            ]
        },
    }


class Closure(unittest.TestCase):
    def test_closure_follows_normal_and_build_not_dev(self):
        m = meta(
            {"cyanotype-cli": None, "core": None, "auth": None, "devonly": None, "serde": "registry"},
            {"cyanotype-cli": [("core", None), ("devonly", "dev"), ("serde", None)], "core": [("serde", "build")]},
        )
        dirs, fp = r.closure(m, "/w")
        self.assertEqual(dirs, ["core", "cyanotype-cli"])
        self.assertEqual(sorted(f[0] for f in fp), sorted(["cyanotype-cli", "core", "serde"]))

    def test_decide(self):
        dirs = ["apps/cli", "crates/cyanotype-core"]
        fp = [["serde", "1.0.0", "registry", []]]
        self.assertTrue(r.decide(["apps/cli/src/main.rs"], dirs, fp, fp))
        self.assertTrue(r.decide(["crates/cyanotype-core/src/lib.rs"], dirs, fp, fp))
        self.assertFalse(r.decide(["apps/cli-other/x.rs", "docs/x.md", "apps/web/a.ts", "Cargo.lock"], dirs, fp, fp))
        # Cargo.lock bumped a dependency of the CLI.
        self.assertTrue(r.decide(["Cargo.lock"], dirs, fp, [["serde", "1.0.1", "registry", []]]))


class MetadataFailure(unittest.TestCase):
    """A cargo that cannot resolve must never read as "the CLI changed"."""

    def _fake(self, codes):
        calls = []

        def fake(args, directory):
            calls.append(args[0])
            code = codes.pop(0)
            return subprocess.CompletedProcess(args, code, stdout="{}" if code == 0 else "", stderr="boom")

        return calls, fake

    def test_raises_when_fetch_fails(self):
        calls, fake = self._fake([101, 101])
        with mock.patch.object(r, "_cargo", fake), self.assertRaises(r.MetadataError):
            r.metadata_at("/w")
        self.assertEqual(calls, ["metadata", "fetch"])

    def test_raises_when_still_failing_after_fetch(self):
        calls, fake = self._fake([101, 0, 101])
        with mock.patch.object(r, "_cargo", fake), self.assertRaises(r.MetadataError):
            r.metadata_at("/w")
        self.assertEqual(calls, ["metadata", "fetch", "metadata"])

    def test_fetches_and_retries_when_crates_missing(self):
        calls, fake = self._fake([101, 0, 0])
        with mock.patch.object(r, "_cargo", fake):
            self.assertEqual(r.metadata_at("/w"), {})
        self.assertEqual(calls, ["metadata", "fetch", "metadata"])

    def test_changed_propagates_instead_of_returning_true(self):
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
        with mock.patch.object(r, "metadata_at", side_effect=r.MetadataError("x")):
            with self.assertRaises(r.MetadataError):
                r.changed(head, head, ROOT)

    def test_cli_exits_nonzero_with_message(self):
        err = io.StringIO()
        with mock.patch.object(r, "changed", side_effect=r.MetadataError("no crates")), \
                mock.patch.object(sys, "stderr", err), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(r.main(["x", "changed", "a", "b"]), 1)
        self.assertEqual(out.getvalue(), "")
        self.assertIn("cannot tell whether the CLI changed", err.getvalue())
        self.assertIn("no crates", err.getvalue())


@unittest.skipUnless(shutil.which("cargo"), "needs cargo")
class RealWorkspace(unittest.TestCase):
    def test_this_repo(self):
        m = r.metadata_at(ROOT)
        dirs, _ = r.closure(m, ROOT)
        self.assertIn("apps/cli", dirs)
        self.assertIn("crates/cyanotype-core", dirs)
        self.assertNotIn("apps/auth", dirs)
        self.assertNotIn("apps/worker", dirs)

    def test_same_commit_is_unchanged(self):
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
        self.assertFalse(r.changed(head, head, ROOT))


if __name__ == "__main__":
    unittest.main()
