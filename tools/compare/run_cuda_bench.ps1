# Run Rust ort bench with CUDA EP search paths set for this machine.
# Usage:
#   .\tools\compare\run_cuda_bench.ps1
#   .\tools\compare\run_cuda_bench.ps1 -Onnx resources\models\phase2_rev1.onnx -WindowsDir output\bench_windows -Provider cuda

param(
    [string]$Onnx = "resources\models\phase2_smoke.onnx",
    [string]$WindowsDir = "output\bench_windows",
    [ValidateSet("auto", "cpu", "cuda", "coreml")]
    [string]$Provider = "cuda",
    [int]$Warmup = 3
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $Repo

$env:CUDA_PATH = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4"
$cudnn = "C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64"
$env:ORT_CUDA_VERSION = "13"
$env:PATH = "$cudnn;$env:CUDA_PATH\bin;$env:CUDA_PATH\bin\x64;$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:CARGO_TARGET_DIR = Join-Path $Repo "target"

if (-not (Test-Path $Onnx)) {
    Write-Error "ONNX not found: $Onnx (place phase2_rev1.onnx or run: python tools/export/make_smoke_onnx.py)"
}

$vsdev = "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat"
$cmd = @"
call "$vsdev" -arch=x64 -host_arch=x64 && cargo run --release --example bench_infer -- "$Onnx" "$WindowsDir" $Provider $Warmup
"@
cmd /c $cmd
