#!/usr/bin/env python3
"""
Generate docs/boards/VALIDATION_STATUS.md from validation/manifest.yaml and
enforce the no-silent-decay rule.

The board docs drift; this generated table does not. It is the single
machine-checked view of what is actually validated, against which silicon, and
on what date — plus an automated DRIFT gate that catches the case where a
peripheral model changed AFTER the board's last silicon capture.

Modes
-----
  (default)      regenerate docs/boards/VALIDATION_STATUS.md in place
  --check        regenerate to memory and diff against the committed file;
                 exit 1 if they differ (run this in CI so the doc cannot go stale)
  --drift        exit 1 if any silicon-tier board has DRIFTED past its drift_ack
  (you normally run CI with BOTH:  --check --drift)

Either gate flag also runs the drift-watch COVERAGE audit (see below).

Drift
-----
For each board with `silicon.last_capture`, a content digest over everything
`models` watches is compared to `silicon.models_digest` — the digest recorded
when that silicon capture was taken. Different digest means the board has
drifted. A `drift_ack_digest` naming the current content is an explicit human
acknowledgement that keeps it green; any later model change moves the digest and
re-breaks the gate.

DRIFT IS JUDGED ON CONTENT, NOT COMMIT DATES. It used to take the newest git
committer date across `models` and compare it to the capture date, which made
the gate depend on a timestamp no PR author can see or control: squash-merging
rewrites every touched file's date to the merge commit's, so a PR could pass
`pr-gate` with a covering ack and then red `main` on identical content, and
every branch cut afterwards inherited the failure (#834). A squash timestamp is
not evidence about whether a model changed; the bytes are. Digests survive
squash, rebase and cherry-pick untouched, and the value CI computes is the value
you can compute locally before you push:

    python3 scripts/generate_validation_status.py --digests

The same change takes the volatile dates out of the generated doc, which used to
need a regen commit after every merge that touched a watched path.

Drift-watch coverage
--------------------
The drift gate can only see what `models` lists, and an incomplete list fails
OPEN: the board reads "fresh" forever while the files its claim rests on change
underneath. esp32c3 shipped that way — its tier is a reset-state oracle asserted
against the declarative descriptors in `configs/peripherals/esp32c3/`, and all
29 of them were outside its watch list, as was the shared `esp_uart.rs` its real
UART0/UART1 register map moved into on 2026-07-28.

So we audit the watch list itself, mechanically:
  * every `path:` a board's chip yaml wires (resolved relative to the chip yaml)
    must be covered by an entry in that board's `models`; and
  * every listed `models` path must exist — a stale path is a silently disabled
    watch, not a warning.
Coded (non-declarative) peripheral impls cannot be derived from the yaml and are
still listed by hand; this audit closes the mechanical half of the hole.

Needs PyYAML (pip install pyyaml). A shallow checkout is fine — nothing here
reads git history any more.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import posixpath
import re
import subprocess
import sys
from datetime import date, datetime
from pathlib import Path

try:
    import yaml
except ImportError:
    print("ERROR: PyYAML required — `pip install pyyaml`", file=sys.stderr)
    sys.exit(2)

CORE_ROOT = Path(__file__).resolve().parent.parent
MANIFEST = CORE_ROOT / "validation" / "manifest.yaml"
OUT_DOC = CORE_ROOT / "docs" / "boards" / "VALIDATION_STATUS.md"

TIER_BADGE = {
    "silicon-verified": "🟢 silicon-verified",
    "silicon-smoke": "🟢 silicon-smoke",
    "sim-validated": "🔵 sim-validated (deep model, no HW diff)",
    "smoke-manual": "🟡 smoke-manual",
    "structural": "⚪ structural",
}


def parse_iso(v: str) -> datetime:
    """datetime.fromisoformat, but accepting a trailing 'Z' for UTC.

    Only Python 3.11+ accepts that suffix, so on the macOS system interpreter
    (3.9) every local run of this script died while CI's 3.12 passed — the
    regeneration command the error message itself tells you to run was
    impossible to run on a stock Mac. Normalise instead of requiring 3.11.
    """
    return datetime.fromisoformat(v[:-1] + "+00:00" if v.endswith("Z") else v)


# `path: "../peripherals/esp32c3/system.yaml"` inside a chip yaml.
CHIP_YAML_PATH_RE = re.compile(r"""^\s*path:\s*["']?([^"'\s#]+)["']?\s*(?:#.*)?$""", re.M)


def covers(model_entry: str, path: str) -> bool:
    """True if a `models` entry (file or directory) covers `path`."""
    return path == model_entry or path.startswith(model_entry.rstrip("/") + "/")


def watch_gaps(board: dict) -> tuple[list[str], list[str]]:
    """(uncovered chip-yaml paths, listed model paths that do not exist).

    Both are drift-gate holes that fail OPEN — the board keeps reading "fresh"
    while something its claim depends on is unwatched. See module docs.
    """
    models = board.get("models", [])
    missing = [m for m in models if not (CORE_ROOT / m).exists()]

    chip_rel = board.get("chip")
    uncovered: list[str] = []
    if chip_rel and (CORE_ROOT / chip_rel).exists():
        chip_dir = Path(chip_rel).parent
        for raw in CHIP_YAML_PATH_RE.findall((CORE_ROOT / chip_rel).read_text()):
            # Chip yamls reach configs/peripherals via `../`; normalise textually
            # (posixpath.normpath, not resolve()) so the result is repo-relative
            # and stable regardless of where the checkout lives or symlinks.
            wired = posixpath.normpath(posixpath.join(chip_dir.as_posix(), raw))
            if not any(covers(m, wired) for m in models):
                uncovered.append(wired)
    return sorted(set(uncovered)), missing


def audit_watch_lists(manifest: dict) -> int:
    """Fail the build on any drift-watch hole. Returns 0 or 1."""
    rc = 0
    for b in manifest["boards"]:
        uncovered, missing = watch_gaps(b)
        if missing:
            print(
                f"ERROR: {b['id']}: `models` lists path(s) that do not exist — a stale "
                f"entry watches nothing:\n  " + "\n  ".join(missing),
                file=sys.stderr,
            )
            rc = 1
        if uncovered:
            print(
                f"ERROR: {b['id']}: {len(uncovered)} path(s) wired by {b['chip']} are NOT "
                "covered by its `models` drift-watch list, so a change to them cannot "
                "fail the drift gate:\n  " + "\n  ".join(uncovered) + "\n"
                "       Add them (a parent directory counts) to validation/manifest.yaml.",
                file=sys.stderr,
            )
            rc = 1
    return rc


NO_MODELS_DIGEST = "-" * 16


def watched_files(paths: list[str]) -> list[str]:
    """Every tracked file under the given `models` entries, repo-relative, sorted.

    The file LIST comes from the index (`git ls-files`) rather than a directory
    walk, so an untracked scratch file dropped into a watched directory cannot
    move a board's digest. The file CONTENT is read from the working tree, so a
    local edit is visible to the gate before you commit it — which is the whole
    point of a digest an author can reproduce.
    """
    if not paths:
        return []
    out = subprocess.run(
        ["git", "ls-files", "-z", "--"] + paths,
        cwd=CORE_ROOT,
        capture_output=True,
        text=True,
    ).stdout
    return sorted(p for p in out.split("\0") if p)


def models_digest(paths: list[str]) -> str:
    """Content digest over everything a board's `models` list watches.

    Path and length are hashed alongside the bytes so that renaming a model, or
    moving content between two watched files, changes the digest — a rename is
    exactly the kind of edit that silently kept its old commit date before.
    """
    files = watched_files(paths)
    if not files:
        return NO_MODELS_DIGEST
    h = hashlib.sha256()
    for rel in files:
        target = CORE_ROOT / rel
        if not target.exists():
            # Tracked but deleted in the working tree. Hash the absence rather
            # than skipping it, so a deletion is drift instead of a no-op.
            h.update(rel.encode() + b"\0deleted\0")
            continue
        blob = target.read_bytes()
        h.update(rel.encode() + b"\0" + str(len(blob)).encode() + b"\0" + blob)
    return h.hexdigest()[:16]


def as_date(v) -> date | None:
    if v is None:
        return None
    if isinstance(v, date):
        return v
    return parse_iso(str(v)).date()


def evaluate(board: dict) -> dict:
    """Compute drift status for one board."""
    silicon = board.get("silicon")
    models = board.get("models", [])
    current = models_digest(models)
    capture = as_date(silicon["last_capture"]) if silicon else None
    captured_digest = silicon.get("models_digest") if silicon else None
    ack = as_date(board.get("drift_ack"))
    ack_digest = board.get("drift_ack_digest")

    # A board whose capture predates digests has no recorded baseline to compare
    # against, so freshness cannot be PROVEN — treat it as drifted and let the
    # ack carry it until the next live re-capture records a digest. Failing open
    # ("assume fresh") is the one answer this gate must never give.
    drifted = bool(silicon) and captured_digest != current
    acked = bool(ack_digest) and ack_digest == current
    # A board with no silicon capture cannot "drift" — it never claimed silicon.
    failing = drifted and not acked

    if not silicon:
        status = "no silicon capture"
    elif not drifted:
        status = "✅ fresh"
    elif acked:
        when = f" {ack:%Y-%m-%d}" if ack else ""
        status = f"⚠ drift acked{when} (re-capture pending)"
    elif captured_digest is None:
        status = "✖ DRIFT — capture recorded no models_digest; RE-CAPTURE"
    else:
        status = "✖ DRIFT — models changed since capture; RE-CAPTURE"

    return {
        "digest": current,
        "capture": capture,
        "drifted": drifted,
        "failing": failing,
        "status": status,
    }


def render(manifest: dict) -> str:
    boards = manifest["boards"]
    lines: list[str] = []
    lines.append("<!-- GENERATED by scripts/generate_validation_status.py — DO NOT EDIT BY HAND.")
    lines.append("     Source of truth: validation/manifest.yaml. Regenerated and gated on every CI run. -->")
    lines.append("")
    lines.append("# Board validation status")
    lines.append("")
    lines.append(
        "Machine-generated from `validation/manifest.yaml`. CI regenerates this on "
        "every run (`--check`) and fails if a peripheral model changed after a "
        "board's last silicon capture without a covering `drift_ack_digest` "
        "(`--drift`). Tiers: 🟢 silicon · 🟡 manual-smoke · ⚪ structural."
    )
    lines.append("")
    lines.append(
        "The models column is a content digest over everything that board's "
        "`models` list watches. It is derived from the bytes, not from commit "
        "dates, so it does not move when a squash merge re-dates the files."
    )
    lines.append("")
    lines.append("| Board | Tier | Last silicon capture | Models | Status |")
    lines.append("|-------|------|----------------------|--------|--------|")
    for b in boards:
        ev = evaluate(b)
        tier = TIER_BADGE.get(b["tier"], b["tier"])
        cap = f"{ev['capture']:%Y-%m-%d}" if ev["capture"] else "—"
        digest = f"`{ev['digest']}`" if b.get("models") else "—"
        lines.append(f"| `{b['id']}` | {tier} | {cap} | {digest} | {ev['status']} |")
    lines.append("")

    # Per-board detail
    for b in boards:
        ev = evaluate(b)
        lines.append(f"## `{b['id']}` — {TIER_BADGE.get(b['tier'], b['tier'])}")
        lines.append("")
        lines.append(f"- Doc: [`{b['doc']}`]({Path(b['doc']).name})  ·  Chip: `{b['chip']}`")
        if b.get("note"):
            lines.append(f"- Note: {b['note']}")
        sil = b.get("silicon")
        if sil:
            lines.append(
                f"- Silicon: **{ev['capture']:%Y-%m-%d}** on {sil.get('probe', '?')} — {sil.get('result', '')}"
            )
        else:
            lines.append("- Silicon: none — not validated against real hardware.")
        for t in b.get("offline_tests", []):
            lines.append(f"  - offline (CI): {t}")
        if b.get("models"):
            watched = len(watched_files(b["models"]))
            lines.append(f"- Models watched: {watched} file(s), digest `{ev['digest']}`")
        lines.append(f"- Drift status: **{ev['status']}**")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="fail if committed doc is stale")
    ap.add_argument("--drift", action="store_true", help="fail if any board drifted past its ack")
    ap.add_argument(
        "--digests",
        action="store_true",
        help="print each board's current models digest (paste into the manifest) and exit",
    )
    args = ap.parse_args()

    manifest = yaml.safe_load(MANIFEST.read_text())

    if args.digests:
        try:
            for b in manifest["boards"]:
                if b.get("models"):
                    print(f"{b['id']:<28} {models_digest(b['models'])}")
        except BrokenPipeError:
            # `--digests | head` / `| grep -m1` is the obvious way to use this;
            # it should not end in a traceback.
            sys.stdout = None
        return 0

    rendered = render(manifest)

    rc = 0

    # Coverage before content: a stale doc is visible, an unwatched model is not.
    if args.check or args.drift:
        rc |= audit_watch_lists(manifest)

    if args.check:
        existing = OUT_DOC.read_text() if OUT_DOC.exists() else ""
        if existing != rendered:
            # Print the actual difference: "is out of date" on its own gives a
            # CI reader nothing to act on.
            diff = "".join(
                difflib.unified_diff(
                    existing.splitlines(keepends=True),
                    rendered.splitlines(keepends=True),
                    fromfile=f"{OUT_DOC.name} (committed)",
                    tofile=f"{OUT_DOC.name} (regenerated here)",
                )
            )
            print(
                f"ERROR: {OUT_DOC.relative_to(CORE_ROOT)} is out of date.\n"
                "       Run: python3 scripts/generate_validation_status.py\n"
                f"{diff}",
                file=sys.stderr,
            )
            rc = 1
    elif not args.drift:
        # Pure generate mode (no gate flags): rewrite the doc in place.
        OUT_DOC.write_text(rendered)
        print(f"wrote {OUT_DOC.relative_to(CORE_ROOT)}")

    if args.drift:
        failing = [b for b in manifest["boards"] if evaluate(b)["failing"]]
        if failing:
            # Name the digest each board needs. It is the value the fix has to
            # contain, it is stable under squash merge, and it is reproducible
            # locally with --digests — so a CI reader can act without guessing.
            detail = "\n  ".join(f"{b['id']}: {models_digest(b['models'])}" for b in failing)
            print(
                "ERROR: silicon validation has DRIFTED (watched models differ from the "
                "content captured against silicon, no covering drift_ack_digest):\n  "
                + detail
                + "\n"
                "       Re-run the live diff and bump silicon.last_capture + "
                "silicon.models_digest, or record the digest above as drift_ack_digest "
                "(with a dated drift_ack) in validation/manifest.yaml.\n"
                "       Reproduce locally: "
                "python3 scripts/generate_validation_status.py --digests",
                file=sys.stderr,
            )
            rc = 1

    return rc


if __name__ == "__main__":
    sys.exit(main())
