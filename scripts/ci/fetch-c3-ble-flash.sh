#!/usr/bin/env bash
# Fetch + compose the ESP32-C3 Arduino-BLE probe flash image that the
# `esp32c3_ble_arduino_reaches_ble_init_and_advertising` gate boots.
#
# WHY THIS EXISTS
# ---------------
# The gate needs a REAL Arduino-ESP32 BLE binary — the whole point is that the
# twin runs the compiled Bluedroid stack, not a stand-in. That binary is a 4 MiB
# composed flash image. Committing it would put a stale-able 4 MiB blob in git;
# building it here would mean installing PlatformIO + the ESP32 Arduino core
# into a Rust job to reproduce firmware that already exists. So: fetch the four
# content-addressed flash parts, verify every digest, and compose them locally.
#
# Digests are pinned in scripts/ci/c3-ble-flash.sha256. A mismatch is LOUD and
# fatal — silently accepting a different binary would mean the gate stops being
# evidence about this engine, which is the failure mode it exists to prevent.
#
# ⚠ THE DEFAULT SOURCE IS NOT PERMANENT HOSTING
# ------------------------------------------------
# `api.labwired.com/v1/blobs` is the hosted COMPILE-ARTIFACT CACHE, and it
# expires: `BLOB_TTL_SECONDS = 60*60*24*90` in packages/api/src/blobs.ts, with
# `expires_at` enforced on read for the D1 tier and as a KV `expirationTtl` for
# the KV tier. Ninety days after these four parts were last written they stop
# resolving, and this script starts failing (loudly — which is the correct
# failure, but it is still rot on a timer).
#
# The fix is to move these parts onto real hosting and repoint the base URL:
# the deployed playground already serves permanent firmware from
# `https://app.labwired.com/wasm/` (see the superproject's
# scripts/ci/fetch-shipped-lab-flash.sh, same pattern), or any R2 bucket with
# no TTL. Name each object after its sha256 and the URL shape below is
# unchanged — only C3_BLE_BLOB_BASE_URL moves.
#
# Usage: scripts/ci/fetch-c3-ble-flash.sh [dest-dir]
#        (default dest: fixtures/esp32c3-ble/, which .gitignore covers)
# Usage: scripts/ci/fetch-c3-ble-flash.sh [dest-dir] [manifest]
#        default dest:     fixtures/esp32c3-ble/  (.gitignore covers it)
#        default manifest: scripts/ci/c3-ble-flash.sha256
#
# The manifest argument is what lets ONE fetch/compose/verify mechanism serve
# every C3 BLE gate instead of a copy per image. The second manifest in-tree is
# `c3-ble-node-flash.sha256` (the two-way advertise+scan node image the
# `e2e_esp32c3_ble_two_node` gate boots as BOTH nodes):
#
#   scripts/ci/fetch-c3-ble-flash.sh fixtures/esp32c3-ble \
#       scripts/ci/c3-ble-node-flash.sha256
#
# Env:   C3_BLE_BLOB_BASE_URL  override the blob store (default: production)
set -euo pipefail

dest="${1:-fixtures/esp32c3-ble}"
base="${C3_BLE_BLOB_BASE_URL:-https://api.labwired.com/v1/blobs/sha256}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sums="${2:-$here/c3-ble-flash.sha256}"

[ -f "$sums" ] || { echo "missing checksum manifest: $sums" >&2; exit 1; }

# GNU coreutils on CI, BSD/macOS locally. Same digest either way.
if command -v sha256sum >/dev/null 2>&1; then
  _sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  _sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  echo "no sha256sum or shasum on PATH — cannot verify digests, refusing to run" >&2
  exit 1
fi

mkdir -p "$dest"
# Bucket the part cache by manifest: two manifests share part NAMES (`app`) with
# different digests, so one shared directory would make them evict each other on
# every alternating run. The digest check would still catch it — this only
# avoids the pointless re-download.
parts_dir="$dest/parts/$(basename "$sums" .sha256)"
mkdir -p "$parts_dir"

image_sha=""
image_name=""
offsets=()
names=()

while read -r want off name; do
  case "$want" in ''|\#*) continue ;; esac
  if [ "$off" = "image" ]; then
    image_sha="$want"
    image_name="$name"
    continue
  fi

  out="$parts_dir/$name.bin"
  if [ -f "$out" ] && [ "$(_sha256 "$out")" = "$want" ]; then
    echo "ok (cached)   $name"
  else
    if ! curl -fsSL --retry 3 --retry-connrefused -o "$out.tmp" "$base/$want"; then
      echo "FETCH FAILED  $name  ($base/$want)" >&2
      rm -f "$out.tmp"
      exit 1
    fi
    got="$(_sha256 "$out.tmp")"
    if [ "$got" != "$want" ]; then
      # The blob store is content-addressed, so this should be impossible.
      # If it ever happens, something is serving the wrong bytes under the
      # right name and the gate must NOT boot them.
      echo "DIGEST MISMATCH  $name" >&2
      echo "  expected $want" >&2
      echo "  got      $got" >&2
      rm -f "$out.tmp"
      exit 1
    fi
    mv "$out.tmp" "$out"
    echo "ok (fetched)  $name"
  fi
  offsets+=("$off")
  names+=("$out")
done < "$sums"

[ -n "$image_sha" ] || { echo "manifest has no 'image' row: $sums" >&2; exit 1; }

image="$dest/$image_name"
if [ -f "$image" ] && [ "$(_sha256 "$image")" = "$image_sha" ]; then
  echo "ok (cached)   $image_name"
  echo "$image"
  exit 0
fi

# Compose: 4 MiB of erased flash (0xFF), parts written at their offsets. `dd`
# with a 0xFF-filled base is the portable way to do this with no Python
# dependency in the job.
tmp="$image.tmp"
rm -f "$tmp"
# 4 MiB of 0xFF. tr from /dev/zero is portable; `head -c` bounds it exactly.
head -c 4194304 /dev/zero | LC_ALL=C tr '\000' '\377' > "$tmp"
for i in "${!names[@]}"; do
  off=$(( ${offsets[$i]} ))
  # Seek in 4 KiB blocks, not bytes: `dd bs=1` on the ~1 MB app part is a
  # million single-byte writes. Every flash offset here is 4 KiB-aligned
  # (that is a property of esptool layouts, not luck) — assert it rather
  # than assume it, because a misaligned seek would silently place the part
  # at the wrong address and the composed-image digest check below would
  # then be the only thing between us and a mystery.
  if [ $(( off % 4096 )) -ne 0 ]; then
    echo "offset ${offsets[$i]} is not 4 KiB-aligned" >&2
    rm -f "$tmp"
    exit 1
  fi
  dd if="${names[$i]}" of="$tmp" bs=4096 seek=$(( off / 4096 )) conv=notrunc status=none
done

got="$(_sha256 "$tmp")"
if [ "$got" != "$image_sha" ]; then
  echo "COMPOSED IMAGE DIGEST MISMATCH  $image_name" >&2
  echo "  expected $image_sha" >&2
  echo "  got      $got" >&2
  echo "  Every part verified, so the composition (offsets / pad byte / size)" >&2
  echo "  is what changed. Fix it here, do not repin." >&2
  rm -f "$tmp"
  exit 1
fi
mv "$tmp" "$image"
echo "ok (composed) $image_name"
echo "$image"
