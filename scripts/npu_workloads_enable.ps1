# npu_workloads_enable.ps1
# Restore Windows AI (WSAI) workloads to the default (Automatic + Running).
# Reverts what npu_free.ps1 disabled (only use it for experiments).
#
# Usage (Administrator PowerShell):
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\npu_workloads_enable.ps1

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Run as administrator."
}

Write-Host "WSAIFabricSvc -> Automatic + Start"
Set-Service WSAIFabricSvc -StartupType Automatic
Start-Service WSAIFabricSvc

Get-Service WSAIFabricSvc | Format-List Name, Status, StartType
Write-Host "Done. WorkloadsSessionHost restarts as needed (NPU re-occupied)."
