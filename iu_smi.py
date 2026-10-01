#!/usr/bin/env python
"""Python entry point for the Rust-based iu-smi monitor."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
MANIFEST = ROOT / "Cargo.toml"


def openvino_env() -> dict[str, str]:
    env = os.environ.copy()
    if env.get("IU_SMI_OPENVINO_DEVICES"):
        return env

    try:
        import openvino as ov  # type: ignore

        core = ov.Core()
        env["IU_SMI_OPENVINO_DEVICES"] = ",".join(core.available_devices)
        env["IU_SMI_OPENVINO_VERSION"] = getattr(ov, "__version__", "unknown")
        env["IU_SMI_OPENVINO_SOURCE"] = "python-openvino-runtime"
    except Exception as exc:  # OpenVINO is optional for launching the TUI.
        env["IU_SMI_OPENVINO_DEVICES"] = ""
        env["IU_SMI_OPENVINO_SOURCE"] = f"python-openvino-unavailable:{type(exc).__name__}"
    return env


def _infer_mem_type(speed_mts: int | None) -> str | None:
    """Infer the memory technology from its speed (MT/s). Best-effort for display:
    the 226V (and similar SoCs) use integrated/unified memory, and WMI reports the
    Speed but MemoryType=0 (unknown) + a blank PartNumber, so the name is derived
    from the speed. 8533 MT/s == LPDDR5X-8533."""
    if not speed_mts:
        return None
    if speed_mts >= 7000:
        return "LPDDR5X"
    if speed_mts >= 5500:
        return "LPDDR5"
    if speed_mts >= 4000:
        return "LPDDR4X"
    if speed_mts >= 3000:
        return "LPDDR4"
    if speed_mts >= 2000:
        return "LPDDR3"
    return "DRAM"


def read_memory_info() -> dict:
    """Read physical-memory details (speed, manufacturer, capacity, module count)
    via WMI (`Win32_PhysicalMemory`). Dynamic — reads whatever the current machine
    reports. The technology name (`type`) is inferred from the speed (see
    `_infer_mem_type`) because WMI reports MemoryType=0 on the 226V."""
    info: dict = {
        "speed_mts": None, "manufacturer": None, "modules": 0,
        "capacity_bytes": 0, "type": None, "human": None,
    }
    if os.name != "nt":
        return info
    try:
        import json as _json
        import subprocess as _sp
        cmd = [
            "powershell", "-NoProfile", "-Command",
            "Get-CimInstance Win32_PhysicalMemory | "
            "Select-Object Speed,Manufacturer,Capacity | ConvertTo-Json -Compress",
        ]
        out = _sp.run(cmd, capture_output=True, text=True, timeout=20).stdout
        rows = _json.loads(out) if out.strip() else []
        if isinstance(rows, dict):
            rows = [rows]
        speeds = [int(r["Speed"]) for r in rows if r.get("Speed")]
        mfrs = sorted({str(r.get("Manufacturer") or "").strip() for r in rows
                       if str(r.get("Manufacturer") or "").strip()})
        caps = [int(r["Capacity"]) for r in rows if r.get("Capacity")]
        if speeds:
            info["speed_mts"] = speeds[0]
            info["type"] = _infer_mem_type(speeds[0])
        if mfrs:
            info["manufacturer"] = "/".join(mfrs)
        info["modules"] = len(rows)
        info["capacity_bytes"] = sum(caps) if caps else 0
        parts: list[str] = []
        if info["type"] and info["speed_mts"]:
            parts.append(f"{info['type']}-{info['speed_mts']}")
        elif info["speed_mts"]:
            parts.append(f"{info['speed_mts']} MT/s")
        if info["manufacturer"]:
            parts.append(info["manufacturer"])
        parts.append("unified")
        info["human"] = " ".join(parts)
    except Exception:
        pass
    return info


def _wants_json(rest: list[str]) -> bool:
    if not rest or rest[0] != "snapshot":
        return False
    for i, a in enumerate(rest):
        if a == "--format" and i + 1 < len(rest) and rest[i + 1] == "json":
            return True
    return any(a == "--format=json" for a in rest)


def _augment_json_text(text: str) -> str:
    """Augment one-shot JSON output with memory details (WMI) + the chip name.
    The Rust binary's schema is fixed (and not rebuildable here under Device Guard),
    so the wrapper adds: memory.{speed_mts,manufacturer,modules,type,human,
    total_capacity_bytes} and a top-level `chip` (= cpu.model). Non-JSON passes
    through unchanged."""
    import json as _json
    mem = read_memory_info()
    chip = None

    def aug(obj):
        if not isinstance(obj, dict):
            return obj
        m = obj.get("memory")
        if isinstance(m, dict):
            m["speed_mts"] = mem["speed_mts"]
            m["manufacturer"] = mem["manufacturer"]
            m["modules"] = mem["modules"]
            m["type"] = mem["type"]
            m["human"] = mem["human"]
            if mem["capacity_bytes"]:
                m["total_capacity_bytes"] = mem["capacity_bytes"]
        cpu = obj.get("cpu")
        if isinstance(cpu, dict) and cpu.get("model") and not obj.get("chip"):
            obj["chip"] = cpu["model"]
        return obj

    s = text.strip()
    try:
        return _json.dumps(aug(_json.loads(s)), indent=2) + "\n"
    except Exception:
        pass
    lines, out, any_obj = s.splitlines(), [], False
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            out.append(_json.dumps(aug(_json.loads(line))))
            any_obj = True
        except Exception:
            out.append(line)
    return ("\n".join(out) + "\n") if any_obj else text


def candidate_binary() -> Path | None:
    exe_name = "iu-smi.exe" if os.name == "nt" else "iu-smi"
    for path in (
        ROOT / exe_name,                        # shipped prebuilt (committed, preferred)
        ROOT / "target" / "release" / exe_name,
        ROOT / "target" / "debug" / exe_name,
    ):
        if path.exists():
            return path
    return None


def run_build(release: bool) -> int:
    cargo = shutil.which("cargo")
    if not cargo:
        print("error: cargo is not installed or not on PATH", file=sys.stderr)
        print("install Rust from https://rustup.rs, then run: python iu_smi.py --build", file=sys.stderr)
        return 127
    cmd = [cargo, "build", "--manifest-path", str(MANIFEST)]
    if release:
        cmd.append("--release")
    return subprocess.run(cmd, cwd=ROOT).returncode


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--debug-build", action="store_true")
    known, rest = parser.parse_known_args(argv)

    if known.build or known.debug_build:
        return run_build(release=not known.debug_build)

    env = openvino_env()
    binary = candidate_binary()
    if binary:
        if _wants_json(rest):
            proc = subprocess.run([str(binary), *rest], cwd=ROOT, env=env,
                                  capture_output=True, text=True)
            if proc.stderr:
                sys.stderr.write(proc.stderr)
            sys.stdout.write(_augment_json_text(proc.stdout))
            return proc.returncode
        return subprocess.run([str(binary), *rest], cwd=ROOT, env=env).returncode

    cargo = shutil.which("cargo")
    if cargo:
        cmd = [cargo, "run", "--quiet", "--manifest-path", str(MANIFEST), "--", *rest]
        return subprocess.run(cmd, cwd=ROOT, env=env).returncode

    print("error: Rust binary is not built and cargo is not available.", file=sys.stderr)
    print("run .\\build.ps1 after installing Rust, or run python iu_smi.py --build.", file=sys.stderr)
    return 127


if __name__ == "__main__":
    raise SystemExit(main())
