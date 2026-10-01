# npu_free.ps1
# NPU free mode: disable Windows AI (WSAI) workloads (Disabled + Kill).
#
# WSAIFabricSvc + WorkloadsSessionHost* keep the NPU at ~20% at idle
# ("NPU leakage"; root cause 2026-09-09: WindowsWorkload.Manager stack,
# EP = WindowsWorkload.EP.Intel.OpenVINO). This turns that off so the NPU
# is dedicated to your experiments.
# Manual alone is not enough (the framework can explicitly restart the
# service), so use Disabled (the default here) + Kill on run for a double
# block. Run once before starting experiments.
#
# Usage (run once from an Administrator PowerShell):
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\npu_free.ps1
# To revert:
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\npu_workloads_enable.ps1
#
# Verify:
#   iu-smi snapshot  -> expect "npu": {"utilization": ~0}

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Run as administrator."
}

Write-Host "[1/3] WSAIFabricSvc -> Stop + Disabled"
Stop-Service WSAIFabricSvc -Force
Set-Service WSAIFabricSvc -StartupType Disabled

Write-Host "[2/3] Terminate WorkloadsSessionHost (release NPU L0 context)"
taskkill /F /IM WorkloadsSessionHost.exe 2>&1 | Out-Null

Write-Host "[3/3] Status"
Get-Service WSAIFabricSvc | Format-List Name, Status, StartType
$left = (Get-Process WorkloadsSessionHost -ErrorAction SilentlyContinue | Measure-Object).Count
Write-Host "Remaining WorkloadsSessionHost: $left"
if ($left -gt 0) {
    Write-Warning "Some session hosts remain; a reboot removes them. Check the NPU%."
}
Write-Host "Done. Verify: iu-smi snapshot (expect NPU ~0%)."
