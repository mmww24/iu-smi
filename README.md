# iu-smi — Unofficial Intel Ultra Chip SMI

> **Unofficial** `nvidia-smi` / `all-smi`-style monitor for **Intel Core Ultra** SoCs
> (Lunar Lake). It is NOT an official Intel product.
>
> **Validation scope:** validated **only** on an **Intel Core Ultra 5 226V, 16 GB
> (Lunar Lake)** machine. Behavior on other Intel Core Ultra parts, memory sizes, or OS
> builds is not verified.

`iu-smi` is a Rust-based monitor shaped after [`nvidia-smi`](https://docs.nvidia.com/deploy/nvidia-smi/),
[`nvitop`](https://github.com/xylont/nvitop), and
[`all-smi`](https://github.com/lablup/all-smi). It is launched through a small Python
wrapper so you can run it with Python while still using a native terminal UI.

## Validation environment

| Component | Version |
|-----------|---------|
| CPU | Intel Core Ultra 5 226V (Lunar Lake) |
| NPU driver | Intel AI Boost **32.0.100.4724** (2026-03-19) |
| GPU driver | Intel Arc 130V **32.0.101.8626** (2026-03-11) |
| OpenVINO (venv) | **2024+** (tested **2024.4.0** + **2026.4.0** `2026.4.0-22959`) |
| OS | Windows 11, build **10.0.26200.0** |

## What it measures — and what it can / cannot observe

iu-smi reads **Windows performance counters (PDH)** + Win32 + (optionally) OpenVINO.
It is a *utilization* monitor, not a power/frequency monitor.

**Observable:**
- **CPU** utilization + CPU model; **system memory** (total / used).
- **Memory type / speed / modules** (e.g. `LPDDR5X-8533`, module count) — via the Python
  wrapper (WMI `Win32_PhysicalMemory`), **JSON output only** (the TUI shows total/used).
- **Intel Xe2 GPU** (Arc 130V) utilization + **per-engine** breakdown
  (`3d`, `compute`, `copy`, `videodecode`, `videoprocessing`, `gsc`, `other`).
- **GPU adapter** shared / dedicated memory.
- **Intel NPU (AI Boost)** utilization — **only when the `engtype_neural` PDH engine
  counter is exposed** (see `npu.engine_seen` below).
- **OpenVINO** device availability (NPU / GPU / CPU) via the Python wrapper
  (`openvino.Core().available_devices`); see `npu.openvino_ready`.
- **Per-process** device utilization (with `--processes`).

**NOT observable from userspace (reported as `null` / unavailable):**
- **CPU / GPU / NPU power** — no userspace PDH power/energy counters.
- **CPU / GPU / NPU frequency** — no userspace frequency counters (Level Zero device
  handles are unreachable).
- **NPU internal engine breakdown** — the NPU is a single aggregate utilization
  (unlike the GPU's per-engine view).
- **Per-device (CPU / GPU / NPU) memory usage** — the 226V uses **unified LPDDR5X memory
  shared by CPU/GPU/NPU** (a single pool, no separate per-device memory, no inter-device
  copies). The reported `memory` is the **system-level** total/used of that one shared
  pool, and `gpu.shared_memory_bytes` / `gpu.dedicated_memory_bytes` is the **shared-pool
  size** the iGPU can address (`dedicated` is typically `0` for an integrated GPU) —
  **not** the GPU's live usage. There is **no userspace counter that attributes exact
  memory usage to each of CPU / GPU / NPU**, so **per-device memory attribution is not
  available** — you cannot say precisely how much memory each device is using.

**Practical implication:** *latency/time is the only QoS observable.* Power- or
frequency-gated experiments are not possible on this box from userspace.

**Useful status flags in the JSON:**
- `npu.engine_seen` — whether the NPU `engtype_neural` PDH counter was exposed
  (if `false`, `npu.utilization` is not meaningful).
- `npu.openvino_ready` — whether OpenVINO reported the NPU as available.
- `gpu.available` — whether the GPU was detected.

## How does this compare to Intel XPU Manager / XPU-SMI?

Intel's official [**XPU Manager / XPU-SMI**](https://github.com/intel/xpumanager) is a free,
open-source tool for **monitoring and managing Intel Data Center / Arc Pro (discrete)
GPUs** — on **Ubuntu and Windows Server**. It is feature-rich for discrete GPUs:
power/frequency/temperature telemetry, ECC and fabric status, GPU topology, firmware
updates, power capping, crash logs, and a metric-exporter daemon.

**iu-smi targets a different, complementary niche — the client AI-PC SoC on Windows
client:**

| | Intel XPU Manager / XPU-SMI | iu-smi |
|---|---|---|
| Hardware | Discrete Data Center / Arc Pro dGPU (Gaudi, DCGM, Arc Pro) | Client Core Ultra SoC (integrated Xe2 iGPU + NPU + CPU) |
| NPU (AI Boost) | No | **Yes** — client NPU utilization |
| Platform | Ubuntu, Windows **Server** | Windows **client** (11), laptop / desktop |
| Power / freq / temp | Yes (Level Zero on the dGPU) | No (not readable from userspace on the 226V) |
| Configuration / firmware | Yes (power limits, freq range, firmware update, ECC) | No (read-only monitor) |
| Form | Admin tool + exporter daemon | Lightweight local TUI / CLI / agent-JSON, one-shot |

In short, **iu-smi is not a competitor to XPU Manager** — it monitors what XPU Manager
(data-center dGPU) does not: the **client SoC's NPU (AI Boost) + integrated iGPU + CPU +
memory + per-process**, on a **Windows client** machine, as a lightweight `nvidia-smi` /
`all-smi`-style monitor. (Notably, `all-smi` itself integrates `xpu-smi` for discrete
GPUs; iu-smi covers the client-SoC / NPU side that neither does.)

**Honest limitation:** iu-smi cannot read power / frequency / temperature on the 226V (a
userspace limitation), so it is **not a superset** of XPU Manager — it is a different,
NPU-centric client tool. Because the 226V uses **unified memory**, iu-smi also cannot
attribute exact per-device (CPU/GPU/NPU) memory usage (see above).

## Prerequisites

What you need before running iu-smi:

| Dependency | Required | Notes |
|------------|----------|-------|
| Windows 10 / 11 (client) | Yes | Validation was on Windows 11, build 10.0.26200.0. |
| Intel Core Ultra SoC (Lunar Lake) | Yes for the NPU / iGPU parts | CPU + system memory work on any x86-64 Windows; the NPU (AI Boost) + Xe2 iGPU sections need the Core Ultra SoC. |
| Intel GPU / NPU drivers | Yes for GPU/NPU utilization | Ship with the SoC via Windows Update / Intel drivers (validation: NPU 32.0.100.4724, GPU 32.0.101.8626). Without them the GPU/NPU sections may report unavailable. |
| Python | Yes (for the wrapper) | **3.10–3.12 recommended** (see below). The wrapper itself runs on 3.9+. |
| OpenVINO | Optional | Only for OpenVINO device discovery (`npu.openvino_ready`, device list). The wrapper degrades gracefully without it. |
| Rust (`rustup`) | Only to build | The repo ships a prebuilt `iu-smi.exe`, so no build is needed to run. |

### Python version (by OpenVINO version)

The Python version you pick must match the OpenVINO version you install — each OpenVINO
release ships CPython wheels for a specific Python range. Verified from PyPI (Windows
`win_amd64` wheels):

| OpenVINO | Python (CPython) |
|----------|------------------|
| 2024.3.0 / 2024.4.0 | 3.8 – 3.12 |
| 2024.5.0 / 2024.6.0 | 3.9 – 3.12 |
| 2025.x (e.g. 2025.3.0) | 3.9 – 3.13 |
| 2026.x (e.g. 2026.4.0) | 3.10 – 3.14 (`Requires-Python >=3.10`) |

**Recommendation: Python 3.10–3.12** — the range compatible with **both OpenVINO 2024 and
2026** (and 2025), so a single venv can track either OpenVINO line. (If you only target
OpenVINO 2026, 3.13/3.14 also work; if you only target 2024.3/2024.4, 3.8/3.9 also work.)

```powershell
# example: a venv that works with OpenVINO 2024+ and 2026
py -3.12 -m venv .venv                    # (or python -m venv .venv on 3.10-3.12)
.\.venv\Scripts\pip install --upgrade pip
.\.venv\Scripts\pip install "openvino>=2024.3"   # optional; omit if you don't need device discovery
```

The wrapper needs **no third-party Python packages** of its own (stdlib only); `openvino`
is the only optional dependency (imported for device discovery and skipped if absent).

## Quick start

A virtual environment is provided at `.venv/` (stdlib `venv`; `uv` was not available on
the reference machine) with **Python 3.13** and **openvino 2026.4.0** installed for device
discovery (recreate with Python 3.10–3.12 if you also want to use OpenVINO 2024; see
"Prerequisites"):

```powershell
.\.venv\Scripts\Activate.ps1
python iu_smi.py snapshot --format json
```

The wrapper finds the binary in `target\release\`, then `target\debug\`, then falls back
to `cargo run`. It degrades gracefully if `openvino` is absent (device discovery is just
skipped).

## Build

Install [Rust](https://rustup.rs) first, then build:

```powershell
.\build.ps1
```

The build writes `target\release\iu-smi.exe` and copies it to `iu-smi.exe`.

> **Prebuilt binary + Device Guard caveat:** the repo ships a prebuilt `iu-smi.exe` so it
> runs without a build. It is a **clean build with no personal/local paths embedded**
> (no usernames or machine-specific directories in the binary). Because it is **unsigned**,
> machines with Device Guard / WDAC may block it ("not a valid Win32 application" /
> "blocked by a security app"); if so, add an EDR/WDAC exception (by hash or path) or build
> from source with `.\build.ps1`.

## Security & permissions

- **Read-only telemetry.** iu-smi only *reads* Windows performance counters (PDH), Win32,
  and (optionally) OpenVINO device availability. It **does not** write, configure, update
  firmware, set power/clock limits, or control any process.
- **Does not violate or bypass Windows security.** It does not interact with, weaken, or
  bypass any security mechanism (WDAC / Device Guard, SmartScreen, Windows Defender, Secure
  Boot, or the UMD / Level Zero security boundary). It reads the same public
  performance-counter surface that task-manager-style tools use.
- **Two honest caveats:**
  1. The **prebuilt binary is unsigned**, so a WDAC / Device Guard or SmartScreen *policy*
     on the machine may choose to block it. That is the OS enforcing its own policy (not
     iu-smi bypassing anything); the fix is a normal allowlist entry (by hash or path) or
     building from source.
  2. The **NPU helper scripts** (`scripts/npu_free.ps1`, `scripts/npu_workloads_enable.ps1`)
     stop/start the Windows Workloads (`WSAIFabricSvc`) service and so **require an
     elevated (administrator) shell** — running them from an elevated PowerShell shows the
     standard **User Account Control (UAC)** prompt. They do not touch any security setting;
     they only toggle the WSAI workload service.

## Usage

### GUI (TUI)

`iu-smi` is a terminal UI (TUI). The default mode is **local monitoring** (CPU / GPU / NPU
/ memory / per-process utilization) with colored utilization thresholds:

```powershell
python .\iu_smi.py          # default: local monitor (TUI)
```

| Key | Action |
|-----|--------|
| `q` / `Esc` | Quit |
| `h` / `1` / `F1` | Help |
| `d` / `u` / `g` / `p` / `c` | Sort processes by default / utilization / GPU mem / PID / CPU |
| `f` | Toggle active engine filter |
| `Tab` | Switch focused panel |

### CLI (one-shot)

```powershell
python .\iu_smi.py snapshot --format json        # machine-readable (agent-preferred)
python .\iu_smi.py snapshot --format csv         # long CSV
python .\iu_smi.py snapshot --format prometheus  # Prometheus text
python .\iu_smi.py snapshot --format nvidia      # nvidia-smi-style one-shot
python .\iu_smi.py api --port 9090               # Prometheus HTTP endpoint
python .\iu_smi.py doctor                         # which PDH counters are visible
```

### CLI (time-series, opt-in)

For experiment-side sampling (co-tenant time-series profiling):

- `--interval-ms <ms>`: sub-second sampling (overrides `--interval`); below 1000 ms the
  slow PowerShell `Get-Counter` fallback merge is skipped so the interval stays honest.
- `--processes`: one row per active process per sample (pid, name, unit GPU/NPU, engine,
  utilization).
- `timestamp_ms` (epoch ms) on every sample (JSON + long CSV); human timestamp is ms-precision.
- Prometheus also emits per-engine gauges
  (`iu_smi_gpu_engine_utilization_percent{engine="3d"|...}`) and, with `--processes`,
  per-process gauges (`iu_smi_process_device_utilization_percent{pid=...,unit=...,engine=...}`).

```powershell
python .\iu_smi.py snapshot --format csv --samples 500 --interval-ms 100 --processes
```

### Agent / automation

The **recommended interface is one-shot JSON** — deterministic, machine-parseable, one-shot,
no TTY:

```powershell
python .\iu_smi.py snapshot --format json
```

Schema (one sample):

```jsonc
{
  "timestamp": "2026-10-01 16:00:10.869",
  "timestamp_ms": 1790838010869,
  "hostname": "BOOK-...",
  "chip":   "Intel(R) Core(TM) Ultra 5 226V",
  "cpu":    { "model": "Intel(R) Core(TM) Ultra 5 226V",
              "utilization": 26.29, "frequency_mhz": null, "power_w": null },
  "memory": { "total_bytes": 16687722496, "used_bytes": 8741421056, "utilization": 52.0,
              "type": "LPDDR5X-8533", "speed_mts": 8533, "manufacturer": "Samsung",
              "modules": 8, "human": "16 GB total / 8 GB used",
              "total_capacity_bytes": 17179869184 },
  "gpu":    { "name": "Intel Xe2 integrated GPU", "utilization": 2.34,
              "shared_memory_bytes": 694140928, "dedicated_memory_bytes": 0,
              "engines": { "3d": 2.34, "compute": 0.0, "copy": 0.0, "videodecode": 0.0,
                            "videoprocessing": 0.0, "gsc": 0.0, "other": 0.0 },
              "available": true },
  "npu":    { "name": "Intel AI Boost NPU", "utilization": 0.0,
              "engine_seen": true, "openvino_ready": true },
  "processes": [ /* one entry per active process (with --processes) */ ]
}
```

> `memory.type` / `speed_mts` / `manufacturer` / `modules` / `human` /
> `total_capacity_bytes` and the top-level `chip` are **added by the Python wrapper** (WMI
> `Win32_PhysicalMemory` + the CPU registry `ProcessorNameString`); the raw binary does not
> emit these fields.

Recipes:
- **One-shot probe:** `snapshot --format json` → parse.
- **Time series:** `snapshot --format csv --samples N --interval-ms M --processes`.
- **Prometheus:** `api --port 9090` → scrape `http://localhost:9090`.
- **Counter diagnosis:** `doctor`.

Read `cpu.utilization`, `memory.utilization`, `gpu.utilization`, `npu.utilization` (0–100)
as the headline numbers; use `timestamp_ms` to align a sampled series; check
`npu.engine_seen` before trusting `npu.utilization`.

## NPU helper scripts

`scripts/` manages the Windows AI (WSAI) workloads that hold the NPU at ~20% at idle
("NPU leakage"):
- `scripts/npu_free.ps1` — disable WSAI (dedicate the NPU to experiments).
- `scripts/npu_workloads_enable.ps1` — restore the default (re-enable Windows AI).
- `scripts/README.md` — details + how to verify with `iu-smi snapshot`.

> **Both scripts require an elevated (administrator) shell** — they stop/start the
> `WSAIFabricSvc` Windows service, so running them from an elevated PowerShell shows the
> standard **User Account Control (UAC)** prompt. They do not touch any security setting.
> See "Security & permissions".

## References (command-shape inspiration)

- [all-smi (lablup)](https://github.com/lablup/all-smi) — the UI/command shape intentionally
  follows this project (local-monitoring default, colored thresholds, process-sort shortcuts,
  snapshot output, Prometheus-style API).
- [nvidia-smi (NVIDIA docs)](https://docs.nvidia.com/deploy/nvidia-smi/) — the one-shot
  `nvidia-smi`-style output is modeled on it.
- [nvitop](https://github.com/xylont/nvitop) — terminal-UI monitor inspiration.
- [Intel XPU Manager / XPU-SMI](https://github.com/intel/xpumanager) — Intel's official
  open-source monitor/manager for **Data Center / Arc Pro (discrete) GPUs**. iu-smi is
  **complementary to** it (client SoC + NPU on Windows client), not a replacement — see
  "How does this compare to Intel XPU Manager / XPU-SMI?" above. `all-smi` integrates
  `xpu-smi` for discrete GPUs.

The hardware collector is Intel/Windows-specific and keeps the original local PDH counter
mapping from the earlier C# prototype.
