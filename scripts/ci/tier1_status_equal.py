#!/usr/bin/env python3
"""Exit 0 if two tier1 matrices have identical per-cell statuses (ignoring run_url)."""
import json
import sys


def statuses(path: str) -> dict:
    d = json.loads(open(path).read())
    return {
        chip: {cls: cell.get("status") for cls, cell in row.items()}
        for chip, row in d.items()
    }


def main() -> int:
    try:
        a, b = sys.argv[1], sys.argv[2]
        return 0 if statuses(a) == statuses(b) else 1
    except Exception as e:  # noqa: BLE001
        print(f"tier1_status_equal: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
