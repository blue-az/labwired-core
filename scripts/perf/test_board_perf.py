#!/usr/bin/env python3
# LabWired - Firmware Simulation Platform
# Copyright (C) 2026 Andrii Shylenko
# SPDX-License-Identifier: MIT
"""Tests for the per-board throughput gate's classification and coverage audit.

These cover the two ways #778 stayed open. The gate only ever failed on a
measurement ABOVE its baseline, so the four boards that got ~2x faster in #798
were re-baselined at values roughly 2x too high and kept passing with half the
engine's cost available as free headroom. And `UNCOVERED` was a prose comment
claiming nothing was silently skipped while eight chip descriptors appeared in
neither list.

Nothing here runs valgrind — the classification thresholds and the coverage
audit are pure functions of the lists and the numbers.

Run: python3 -m unittest discover -s scripts -p 'test_*.py'
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "board_perf", Path(__file__).resolve().parent / "board_perf.py"
)
bp = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(bp)


def classify(measured: float, base: float) -> str:
    """The gate's verdict for one board, mirroring the loop in main()."""
    delta = (measured - base) / base
    if delta > bp.REGRESSION_TOLERANCE:
        return "regression"
    if delta < -bp.STALE_BASELINE_TOLERANCE:
        return "stale"
    return "ok"


class ClassificationTest(unittest.TestCase):
    def test_regression_fails(self):
        self.assertEqual(classify(1500.0, 1000.0), "regression")

    def test_small_drift_passes_either_way(self):
        self.assertEqual(classify(1010.0, 1000.0), "ok")
        self.assertEqual(classify(990.0, 1000.0), "ok")

    def test_small_win_does_not_nag(self):
        """A 5% improvement is inside the stale window on purpose."""
        self.assertEqual(classify(950.0, 1000.0), "ok")

    def test_the_778_boards_are_flagged_stale(self):
        """The four boards #798 left un-baselined, at their real measured cost."""
        for board, base, measured in [
            ("stm32h563", 3634.5, 1658.7),
            ("stm32h735", 2729.5, 1430.5),
            ("stm32l073", 3390.5, 1657.5),
            ("stm32wba52", 2380.5, 1241.6),
        ]:
            with self.subTest(board=board):
                self.assertEqual(
                    classify(measured, base),
                    "stale",
                    f"{board} had ~50% of unprotected headroom and the gate said nothing",
                )

    def test_a_stale_baseline_hides_a_real_regression(self):
        """Why stale baselines matter: the old h563 baseline absorbed a 2x regression."""
        old_base, healthy = 3634.5, 1658.7
        doubled = healthy * 2
        self.assertEqual(
            classify(doubled, old_base),
            "ok",
            "a board could double its per-step cost and still pass — that is the hole",
        )
        self.assertEqual(classify(doubled, healthy), "regression")

    def test_thresholds_are_ordered(self):
        self.assertLess(
            bp.REGRESSION_TOLERANCE,
            bp.STALE_BASELINE_TOLERANCE,
            "a regression must fail sooner than a baseline is called stale",
        )


class CoverageAuditTest(unittest.TestCase):
    def test_repo_is_fully_classified(self):
        self.assertEqual(bp.audit_coverage(), [])

    def test_every_chip_is_measured_or_explained(self):
        chips = {p.stem for p in (bp.REPO_ROOT / "configs/chips").glob("*.yaml")}
        self.assertTrue(chips, "no chip descriptors found — wrong REPO_ROOT?")
        self.assertEqual(
            chips - set(bp.BOARDS) - set(bp.UNCOVERED),
            set(),
            "a chip in neither list is silently unmeasured while the gate reads as covering it",
        )

    def test_lists_are_disjoint(self):
        self.assertEqual(set(bp.BOARDS) & set(bp.UNCOVERED), set())

    def test_every_uncovered_entry_has_a_reason(self):
        for board, reason in bp.UNCOVERED.items():
            with self.subTest(board=board):
                self.assertTrue(reason.strip(), f"{board} is excused with no reason")

    def test_every_measured_board_has_a_baseline(self):
        """A board with no baseline can only ever pass."""
        import json

        baselines = json.loads(bp.BASELINE_PATH.read_text())
        self.assertEqual(
            set(bp.BOARDS) - set(baselines),
            set(),
            "measured board(s) with no baseline — run board_perf.py --update",
        )

    def test_audit_catches_an_unclassified_chip(self):
        saved = bp.BOARDS[:]
        try:
            bp.BOARDS.remove("stm32l073")
            errors = bp.audit_coverage()
            self.assertTrue(errors)
            self.assertIn("stm32l073", "\n".join(errors))
        finally:
            bp.BOARDS[:] = saved

    def test_audit_catches_a_board_with_no_descriptor(self):
        saved = bp.BOARDS[:]
        try:
            bp.BOARDS.append("stm32-does-not-exist")
            errors = bp.audit_coverage()
            self.assertTrue(errors)
            self.assertIn("stm32-does-not-exist", "\n".join(errors))
        finally:
            bp.BOARDS[:] = saved


if __name__ == "__main__":
    unittest.main()
