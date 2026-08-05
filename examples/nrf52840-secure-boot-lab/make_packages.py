#!/usr/bin/env python3
# LabWired - Firmware Simulation Platform
# SPDX-License-Identifier: MIT
"""Generate the signed OTA demo packages for the nrf52840-secure-boot-lab.

Emits the `uart_injections` YAML block (three 140-byte packages delivered to
uart0) and the golden SHA-256 digest the test script asserts. Re-run after
changing the package format or payloads and paste the output into
secure-boot-smoke.yaml — never hand-edit the byte arrays.

Package format (140 bytes):
    magic[4]      "LWOT"
    version u32 LE
    payload_len u32 LE (64)
    payload[64]
    sig[64]       ECDSA P-256 (r||s) over SHA-256 of the first 76 bytes,
                  signed with the OEM private key below — the "OEM backend".
                  The device holds ONLY the matching public key, in the
                  secure element's data slot 0 (see
                  crates/core/src/peripherals/components/atecc608a.rs).

This is a DEMO key. A real OEM key lives in an HSM/KMS, not a script.
Requires openssl 3.x on PATH.
"""

import hashlib
import subprocess
import sys
import tempfile
from pathlib import Path

OEM_PRIVATE_KEY_PEM = """-----BEGIN EC PRIVATE KEY-----
MHcCAQEEIEbQ52cE4P+xioOTV8JGgXI5KfvmeA1nZqiEtcdJilkMoAoGCCqGSM49
AwEHoUQDQgAEEfcZdu77/LX6ybFs+3hDv2FPwVlGqLKUlPKMzZTTIpv+B7avKzhK
1hRP/Brfkh4KKDjavEHFrYVNBwqGlzPkyQ==
-----END EC PRIVATE KEY-----
"""


def payload(text: str) -> bytes:
    b = text.encode()
    assert len(b) <= 64, "payload too long"
    return b.ljust(64, b"\0")


def header(version: int, body: bytes) -> bytes:
    return b"LWOT" + version.to_bytes(4, "little") + len(body).to_bytes(4, "little") + body


def der_to_raw64(der: bytes) -> bytes:
    """Minimal DER INTEGER×2 parse → raw P1363 r||s (64 bytes)."""
    assert der[0] == 0x30
    i = 2
    if der[1] & 0x80:  # long-form length (never for P-256, but be safe)
        nlen = der[1] & 0x7F
        i = 2 + nlen
    ints = []
    for _ in range(2):
        assert der[i] == 0x02
        ln = der[i + 1]
        v = der[i + 2 : i + 2 + ln]
        ints.append(int.from_bytes(v, "big"))
        i += 2 + ln
    return ints[0].to_bytes(32, "big") + ints[1].to_bytes(32, "big")


def sign(data: bytes, key_path: Path) -> bytes:
    out = subprocess.run(
        ["openssl", "dgst", "-sha256", "-sign", str(key_path)],
        input=data,
        capture_output=True,
        check=True,
    )
    return der_to_raw64(out.stdout)


def package(version: int, text: str, key_path: Path, corrupt_sig: bool = False) -> bytes:
    head = header(version, payload(text))
    sig = bytearray(sign(head, key_path))
    if corrupt_sig:
        sig[0] ^= 0xFF
    return head + bytes(sig)


def yaml_bytes(data: bytes) -> str:
    rows = []
    for i in range(0, len(data), 16):
        rows.append(", ".join(f"0x{b:02X}" for b in data[i : i + 16]))
    return ",\n        ".join(rows)


def be_words(data: bytes) -> str:
    return " ".join(
        f"0x{int.from_bytes(data[i:i+4], 'big'):08X}" for i in range(0, len(data), 4)
    )


def main() -> int:
    with tempfile.TemporaryDirectory() as td:
        key_path = Path(td) / "oem.pem"
        key_path.write_text(OEM_PRIVATE_KEY_PEM)

        pkgs = [
            (
                "v2-accept",
                package(2, "LabWired OTA image v2 (demo payload, not executable)", key_path),
            ),
            (
                "v1-rollback",
                package(1, "LabWired OTA image v1 - authentic but OLD", key_path),
            ),
            (
                "v3-forged",
                package(3, "LabWired OTA image v3 - FORGED signature", key_path, corrupt_sig=True),
            ),
        ]
        print("uart_injections:")
        for name, pkg in pkgs:
            print(f"  # ── {name} ({len(pkg)} bytes) ──")
            print('  - uart: "uart0"')
            print("    trigger: !at_start")
            print("    bytes:")
            print(f"      [{yaml_bytes(pkg)}]")
        print()
        print("# golden SHA-256 of the v2 package header+payload (firmware recomputes):")
        digest = hashlib.sha256(pkgs[0][1][:76]).digest()
        print(f"# hex:   {digest.hex()}")
        print(f"# words: {be_words(digest)}")
        print()
        print("# golden SHA-256 of the v2 payload alone (asserted against the")
        print("# flash update-slot readback in boot 3):")
        pdigest = hashlib.sha256(pkgs[0][1][12:76]).digest()
        print(f"# hex:   {pdigest.hex()}")
        print(f"# words: {be_words(pdigest)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
