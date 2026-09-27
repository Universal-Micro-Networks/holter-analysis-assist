<#
.SYNOPSIS
Run `holter-analysis-assist compare-accel` with CUDA / cuDNN on PATH (Windows).

.DESCRIPTION
Prepends the CUDA Toolkit and cuDNN DLL folders to PATH for this process only,
then runs the already built compare-accel subcommand (no Visual Studio
developer environment is needed). The candidate setting is compared against
the baseline (CPU, FP32, batch size 1, no CUDA tuning). The license is metered
twice per ECL.

Exit code is the one of compare-accel:
  0 = finished, every given threshold passed (or none was given)
  1 = error (license, model / candidate provider unavailable, analysis, I/O)
  2 = a given threshold failed, or invalid arguments (before any analysis)

Procedure: docs/perf/windows-gpu-benchmark.md

.PARAMETER Ecl
ECL paths to compare (one or more, comma separated). Relative paths (here and
in -Model, -ReportDir, -LicenseConfig, -Exe) are resolved from the current
directory; compare-accel itself runs in the repo root.

.PARAMETER Provider
Candidate execution provider: auto | cpu | cuda. Default: cuda.

.PARAMETER BatchSize
Candidate windows per inference call (1..256). Omitted: CLI default (16).

.PARAMETER CudaTf32
Candidate --cuda-tf32 (true|false|on|off|1|0). Omitted: ONNX Runtime default.

.PARAMETER CudaConv1dPad
Candidate --cuda-conv1d-pad-to-nc1d (true|false|on|off|1|0). Omitted: ONNX Runtime default.

.PARAMETER CudaGraph
Candidate --cuda-graph (true|false|on|off|1|0). Omitted: ONNX Runtime default.

.PARAMETER Model
ONNX model for both baseline and candidate. Omitted: CLI default.

.PARAMETER ReportDir
Output directory. Omitted: a new <repo>\output\accel_compare\<yyyyMMdd-HHmmss>
per run (report files are overwritten and stale per-ECL folders remain, so do
not reuse a directory).

.PARAMETER ToleranceSamples
Beat matching tolerance in 500 Hz samples. Omitted: CLI default (40 = 80 ms).

.PARAMETER ProbStride
Compare beat / event probabilities every N-th window. Omitted: CLI default (1).
Use a larger N for multi-day ECLs (~160 KB of memory per kept window).

.PARAMETER MaxWindows
Cap on 20 s windows per ECL.

.PARAMETER MinRhythmWindowAgreement
Threshold (0..1). Passed as --min-rhythm-window-agreement=<value>.

.PARAMETER MinBeatMatchRate
Threshold (0..1). Passed as --min-beat-match-rate=<value>.

.PARAMETER MinBeatClassAgreement
Threshold (0..1). Passed as --min-beat-class-agreement=<value>.

.PARAMETER MaxProbAbsDiff
Threshold (>= 0). Passed as --max-prob-abs-diff=<value>.

.PARAMETER MaxOffsetSamples
Threshold (>= 0). Passed as --max-offset-samples=<value>.

.PARAMETER LicenseConfig
Path to license.ini. Omitted: CLI default (HOLTER_LICENSE_INI or config\license.ini).

.PARAMETER ExtraArgs
Additional compare-accel arguments, e.g. @('--flag=value').

.PARAMETER Exe
CLI executable. Default: <repo>\target\release\holter-analysis-assist.exe

.PARAMETER CudaPath
CUDA Toolkit folder. Default: C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4

.PARAMETER CudnnBin
cuDNN DLL folder. Default: C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64

.PARAMETER WithoutCuda
Do not add CUDA / cuDNN to PATH and remove existing CUDA / cuDNN entries from
PATH (to check that an unavailable candidate provider aborts the comparison).

.PARAMETER DryRun
Print the environment and command without running it.

.PARAMETER Help
Show this help.

.EXAMPLE
.\tools\compare\run_accel_compare.ps1 -Ecl D:\ecl\a.ecl -Model resources\models\phase2_rev1_dynamic.onnx -Provider cuda -BatchSize 16 -CudaConv1dPad on

.EXAMPLE
.\tools\compare\run_accel_compare.ps1 -Ecl D:\ecl\a.ecl -Provider cpu -BatchSize 16 -MinBeatMatchRate 1 -MinBeatClassAgreement 1 -MinRhythmWindowAgreement 1

.EXAMPLE
.\tools\compare\run_accel_compare.ps1 -Ecl D:\ecl\a.ecl -Provider cuda -WithoutCuda
#>
[CmdletBinding(DefaultParameterSetName = "Run")]
param(
    [Parameter(ParameterSetName = "Run", Mandatory = $true, Position = 0)]
    [string[]]$Ecl,

    [Parameter(ParameterSetName = "Run")]
    [ValidateSet("auto", "cpu", "cuda")]
    [string]$Provider = "cuda",

    [Parameter(ParameterSetName = "Run")]
    [ValidateRange(1, 256)]
    [int]$BatchSize,

    [Parameter(ParameterSetName = "Run")]
    [ValidateSet("true", "false", "on", "off", "1", "0")]
    [string]$CudaTf32,

    [Parameter(ParameterSetName = "Run")]
    [ValidateSet("true", "false", "on", "off", "1", "0")]
    [string]$CudaConv1dPad,

    [Parameter(ParameterSetName = "Run")]
    [ValidateSet("true", "false", "on", "off", "1", "0")]
    [string]$CudaGraph,

    [Parameter(ParameterSetName = "Run")]
    [string]$Model,

    [Parameter(ParameterSetName = "Run")]
    [string]$ReportDir,

    [Parameter(ParameterSetName = "Run")]
    [ValidateRange(0, [int]::MaxValue)]
    [int]$ToleranceSamples,

    [Parameter(ParameterSetName = "Run")]
    [ValidateRange(1, [int]::MaxValue)]
    [int]$ProbStride,

    [Parameter(ParameterSetName = "Run")]
    [ValidateRange(1, [int]::MaxValue)]
    [int]$MaxWindows,

    [Parameter(ParameterSetName = "Run")]
    [string]$MinRhythmWindowAgreement,

    [Parameter(ParameterSetName = "Run")]
    [string]$MinBeatMatchRate,

    [Parameter(ParameterSetName = "Run")]
    [string]$MinBeatClassAgreement,

    [Parameter(ParameterSetName = "Run")]
    [string]$MaxProbAbsDiff,

    [Parameter(ParameterSetName = "Run")]
    [string]$MaxOffsetSamples,

    [Parameter(ParameterSetName = "Run")]
    [string]$LicenseConfig,

    [Parameter(ParameterSetName = "Run")]
    [string[]]$ExtraArgs = @(),

    [Parameter(ParameterSetName = "Run")]
    [string]$Exe = "target\release\holter-analysis-assist.exe",

    [Parameter(ParameterSetName = "Run")]
    [string]$CudaPath = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4",

    [Parameter(ParameterSetName = "Run")]
    [string]$CudnnBin = "C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64",

    [Parameter(ParameterSetName = "Run")]
    [switch]$WithoutCuda,

    [Parameter(ParameterSetName = "Run")]
    [switch]$DryRun,

    [Parameter(ParameterSetName = "Help", Mandatory = $true)]
    [switch]$Help
)

if ($PSCmdlet.ParameterSetName -eq "Help") {
    Get-Help $PSCommandPath -Detailed
    exit 0
}

$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$CallerDir = (Get-Location).ProviderPath

# Paths given by the caller are relative to the caller's directory; the CLI
# itself runs in the repo root so its own defaults (model, license.ini) resolve.
function Resolve-CallerPath([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) { return $Path }
    return (Join-Path $CallerDir $Path)
}

# `powershell -File` passes "a.ecl,b.ecl" as one string, so split on commas.
$Ecl = @($Ecl | ForEach-Object { $_ -split "," } | Where-Object { $_.Trim() } |
    ForEach-Object { Resolve-CallerPath $_.Trim() })
if ($Model) { $Model = Resolve-CallerPath $Model }
if ($LicenseConfig) { $LicenseConfig = Resolve-CallerPath $LicenseConfig }
if ($ReportDir) { $ReportDir = Resolve-CallerPath $ReportDir }
if ($PSBoundParameters.ContainsKey("Exe")) {
    $Exe = Resolve-CallerPath $Exe
} else {
    $Exe = Join-Path $Repo $Exe
}

if ($WithoutCuda) {
    $kept = $env:PATH -split ";" | Where-Object { $_ -and ($_ -notmatch "CUDA|CUDNN") }
    $env:PATH = $kept -join ";"
    Remove-Item Env:CUDA_PATH -ErrorAction SilentlyContinue
} else {
    foreach ($dir in @($CudnnBin, (Join-Path $CudaPath "bin"))) {
        if (-not (Test-Path $dir)) {
            Write-Warning "not found: $dir (the CUDA provider may be unavailable; see README 'NVIDIA (CUDA)')"
        }
    }
    $env:CUDA_PATH = $CudaPath
    $env:ORT_CUDA_VERSION = "13"
    $env:PATH = "$CudnnBin;$CudaPath\bin;$CudaPath\bin\x64;$env:PATH"
}

if (-not (Test-Path $Exe)) {
    Write-Error "executable not found: $Exe (build it with: cargo build --release)"
}

if (-not $ReportDir) {
    $ReportDir = Join-Path $Repo ("output\accel_compare\" + (Get-Date -Format "yyyyMMdd-HHmmss"))
}

$cliArgs = @()
if ($LicenseConfig) { $cliArgs += "--license-config=$LicenseConfig" }
$cliArgs += "compare-accel"
$cliArgs += $Ecl
if ($Model) { $cliArgs += "--model=$Model" }
$cliArgs += "--provider=$Provider"
if ($PSBoundParameters.ContainsKey("BatchSize")) { $cliArgs += "--batch-size=$BatchSize" }
if ($CudaTf32) { $cliArgs += "--cuda-tf32=$CudaTf32" }
if ($CudaConv1dPad) { $cliArgs += "--cuda-conv1d-pad-to-nc1d=$CudaConv1dPad" }
if ($CudaGraph) { $cliArgs += "--cuda-graph=$CudaGraph" }
if ($PSBoundParameters.ContainsKey("ToleranceSamples")) { $cliArgs += "--tolerance-samples=$ToleranceSamples" }
if ($PSBoundParameters.ContainsKey("ProbStride")) { $cliArgs += "--prob-stride=$ProbStride" }
if ($PSBoundParameters.ContainsKey("MaxWindows")) { $cliArgs += "--max-windows=$MaxWindows" }
$cliArgs += "--report-dir=$ReportDir"
if ($MinRhythmWindowAgreement) { $cliArgs += "--min-rhythm-window-agreement=$MinRhythmWindowAgreement" }
if ($MinBeatMatchRate) { $cliArgs += "--min-beat-match-rate=$MinBeatMatchRate" }
if ($MinBeatClassAgreement) { $cliArgs += "--min-beat-class-agreement=$MinBeatClassAgreement" }
if ($MaxProbAbsDiff) { $cliArgs += "--max-prob-abs-diff=$MaxProbAbsDiff" }
if ($MaxOffsetSamples) { $cliArgs += "--max-offset-samples=$MaxOffsetSamples" }
$cliArgs += $ExtraArgs

if ($WithoutCuda) {
    Write-Host "CUDA / cuDNN removed from PATH (-WithoutCuda)"
} else {
    Write-Host "CUDA_PATH=$env:CUDA_PATH"
    Write-Host "cuDNN=$CudnnBin"
}
Write-Host "report dir: $ReportDir"
Write-Host ("> {0} {1}" -f $Exe, (($cliArgs | ForEach-Object { if ($_ -match "\s") { "`"$_`"" } else { $_ } }) -join " "))

if ($DryRun) {
    exit 0
}

Push-Location $Repo
try {
    & $Exe @cliArgs
    $code = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $code
