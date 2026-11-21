$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
foreach ($tool in @('node', 'npm', 'cargo', 'rustc')) {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "Missing prerequisite: $tool" }
}
function Invoke-Checked([string]$Program, [string[]]$Arguments) {
  & $Program @Arguments
  if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
Invoke-Checked 'npm' @('install')
Invoke-Checked 'npm' @('test')
Invoke-Checked 'npm' @('run', 'tokens:check')
Invoke-Checked 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml')
Invoke-Checked 'npm' @('run', 'desktop:build')
Write-Host 'Installer: src-tauri/target/release/bundle/nsis'
