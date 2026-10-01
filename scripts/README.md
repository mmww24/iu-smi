# NPU helper scripts

Two PowerShell scripts that manage the Windows AI (WSAI) workloads that keep the
Intel NPU partially occupied at idle.

## The problem: "NPU leakage"

On an Intel Core Ultra (Lunar Lake) machine, the Windows AI workloads
(`WSAIFabricSvc` service + `WorkloadsSessionHost.exe` processes, via the
`WindowsWorkload.EP.Intel.OpenVINO` endpoint) hold the NPU at ~20% utilization at
idle. This is the "NPU leakage" (root-caused 2026-09-09 to the
WindowsWorkload.Manager stack). It steals NPU capacity from your own OpenVINO /
iu-smi measurements.

## `npu_free.ps1`

Disables the WSAI workloads so the NPU is dedicated to your experiments:
- `WSAIFabricSvc` -> **Stop + Disabled**
- Kills any running `WorkloadsSessionHost.exe` (releases the NPU Level Zero context)

Run once from an **Administrator** PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\npu_free.ps1
```

## `npu_workloads_enable.ps1`

Restores the default (re-enables Windows AI features):
- `WSAIFabricSvc` -> **Automatic + Start**

Run from an **Administrator** PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\npu_workloads_enable.ps1
```

## Verify with iu-smi

After `npu_free.ps1`, the NPU should read ~0%:

```powershell
python .\iu_smi.py snapshot --format json
# expect: "npu": { "utilization": ~0 }
```

After `npu_workloads_enable.ps1`, the NPU may climb back toward ~20% at idle.

## Notes

- These scripts only affect the NPU's idle occupancy; they do not change iu-smi.
- Use `npu_free.ps1` right before a measurement session, and
  `npu_workloads_enable.ps1` when you are done and want Windows AI features back.
