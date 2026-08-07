#!/usr/bin/env python3
# LabWired - Firmware Simulation Platform
# Copyright (C) 2026 Andrii Shylenko
# SPDX-License-Identifier: MIT
"""Regression tests for the silicon drift gate in generate_validation_status.py.

The bug these exist to prevent (#834): the gate judged drift by the newest git
COMMITTER DATE across a board's `models`. Squash-merging rewrites those dates to
the merge commit's, so a PR could pass `pr-gate` under a covering ack and then
turn `main` red on byte-identical content — and every branch cut from main
inherited the failure until someone re-acked. The author was gated on a
timestamp that did not exist at review time.

`test_digest_survives_a_squash_style_rewrite` is the one that would have caught
it. It deliberately asserts BOTH halves: that the commit date really did move
(so the scenario is not vacuous) and that the verdict did not.

Run: python3 -m unittest discover -s scripts -p 'test_*.py'
"""

from __future__ import annotations

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "gvs", Path(__file__).resolve().parent / "generate_validation_status.py"
)
gvs = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(gvs)


def git(repo: Path, *args: str, when: str | None = None) -> str:
    env = {"GIT_TERMINAL_PROMPT": "0", "HOME": str(repo), "PATH": "/usr/bin:/bin"}
    if when:
        env["GIT_AUTHOR_DATE"] = when
        env["GIT_COMMITTER_DATE"] = when
    return subprocess.run(
        ["git", *args], cwd=repo, capture_output=True, text=True, env=env, check=True
    ).stdout.strip()


class DriftGateTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "t")

        models = self.repo / "models"
        models.mkdir()
        (models / "uart.rs").write_text("fn uart() {}\n")
        (models / "scb.rs").write_text("fn scb() {}\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "models", when="2026-08-06T22:06:00 +0000")

        # Point the module at the throwaway repo instead of the real checkout.
        self._saved_root = gvs.CORE_ROOT
        gvs.CORE_ROOT = self.repo
        self.addCleanup(self._restore)

    def _restore(self) -> None:
        gvs.CORE_ROOT = self._saved_root
        self._tmp.cleanup()

    def digest(self) -> str:
        return gvs.models_digest(["models"])

    def newest_commit_date(self) -> str:
        return git(self.repo, "log", "-1", "--format=%cI", "--", "models")

    # -- the #834 regression ------------------------------------------------

    def test_digest_survives_a_squash_style_rewrite(self):
        """Re-dating every watched file must not change the drift verdict."""
        before_digest = self.digest()
        before_date = self.newest_commit_date()

        # What a squash merge does to these files: identical content, new commit,
        # new committer date on the day after the branch's own commit.
        git(self.repo, "commit", "-q", "--amend", "--no-edit", when="2026-08-07T03:03:00 +0000")

        self.assertNotEqual(
            before_date,
            self.newest_commit_date(),
            "test is vacuous unless the commit date actually moved",
        )
        self.assertEqual(
            before_digest,
            self.digest(),
            "a squash-style re-date changed the drift verdict — this is #834",
        )

    def test_acked_board_stays_green_across_a_rewrite(self):
        """The end-to-end shape of #834: acked pre-merge, still acked post-merge."""
        board = {
            "id": "b",
            "models": ["models"],
            "silicon": {"last_capture": "2026-06-20"},
            "drift_ack": "2026-08-06",
            "drift_ack_digest": self.digest(),
        }
        self.assertFalse(gvs.evaluate(board)["failing"], "should pass pre-merge")

        git(self.repo, "commit", "-q", "--amend", "--no-edit", when="2026-08-07T03:03:00 +0000")

        self.assertFalse(gvs.evaluate(board)["failing"], "must still pass post-merge")

    # -- the gate must still gate -------------------------------------------

    def test_editing_a_model_moves_the_digest(self):
        before = self.digest()
        (self.repo / "models" / "uart.rs").write_text("fn uart() { /* changed */ }\n")
        self.assertNotEqual(before, self.digest())

    def test_renaming_a_model_moves_the_digest(self):
        """Path is hashed alongside the bytes: a pure rename is still drift."""
        before = self.digest()
        git(self.repo, "mv", "models/uart.rs", "models/usart.rs")
        self.assertNotEqual(before, self.digest())

    def test_untracked_file_does_not_move_the_digest(self):
        before = self.digest()
        (self.repo / "models" / "scratch.rs.orig").write_text("junk\n")
        self.assertEqual(before, self.digest(), "untracked scratch must not red the gate")

    def test_changed_model_without_ack_fails(self):
        board = {
            "id": "b",
            "models": ["models"],
            "silicon": {"last_capture": "2026-06-20", "models_digest": self.digest()},
        }
        self.assertFalse(gvs.evaluate(board)["failing"])
        self.assertEqual(gvs.evaluate(board)["status"], "✅ fresh")

        (self.repo / "models" / "uart.rs").write_text("fn uart() { /* changed */ }\n")
        self.assertTrue(gvs.evaluate(board)["failing"], "changed model with no ack must fail")

    def test_stale_ack_does_not_cover_a_later_change(self):
        board = {
            "id": "b",
            "models": ["models"],
            "silicon": {"last_capture": "2026-06-20"},
            "drift_ack": "2026-08-06",
            "drift_ack_digest": self.digest(),
        }
        self.assertFalse(gvs.evaluate(board)["failing"])

        (self.repo / "models" / "scb.rs").write_text("fn scb() { /* later change */ }\n")
        self.assertTrue(
            gvs.evaluate(board)["failing"],
            "a model change past the ack must re-break the gate — no silent decay",
        )

    def test_capture_without_a_digest_is_not_reported_fresh(self):
        """Freshness it cannot prove is the one answer this gate must never give."""
        board = {"id": "b", "models": ["models"], "silicon": {"last_capture": "2026-06-20"}}
        ev = gvs.evaluate(board)
        self.assertTrue(ev["drifted"])
        self.assertTrue(ev["failing"])

    def test_board_without_silicon_never_drifts(self):
        board = {"id": "b", "models": ["models"]}
        ev = gvs.evaluate(board)
        self.assertFalse(ev["drifted"])
        self.assertFalse(ev["failing"])
        self.assertEqual(ev["status"], "no silicon capture")


if __name__ == "__main__":
    unittest.main()
