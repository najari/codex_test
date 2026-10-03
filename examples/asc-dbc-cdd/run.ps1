param(
    [ValidateSet('comfort', 'engine', 'both')][string]$Sample = 'both',
    [string]$OutputDirectory,
    [string]$Exe
)
$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
if (-not $Exe) { $Exe = Join-Path $taskRoot 'dist/canlog.exe' }
$taskExe = (Resolve-Path -LiteralPath $Exe).Path
$taskSamples = Join-Path $taskRoot 'can_example/vector_samples/2026-10-03/groups/CANoe_13.0.172/CAN/CANSystemDemo'
$taskCases = @(
    @{ Name = 'comfort'; Asc = 'ComfortDiagData'; Dbc = 'Comfort'; Cdd = 'CANSystemDoor'; Channel = 1 },
    @{ Name = 'engine'; Asc = 'EngineDiagData'; Dbc = 'PowerTrain'; Cdd = 'CANSystem'; Channel = 2 }
) | Where-Object { $Sample -eq 'both' -or $_.Name -eq $Sample }
foreach ($taskCase in $taskCases) {
    foreach ($taskInput in @("asc/Logging/$($taskCase.Asc).asc", "dbc/CANdb/$($taskCase.Dbc).dbc", "cdd/CDD/$($taskCase.Cdd).cdd")) {
        if (-not (Test-Path -LiteralPath (Join-Path $taskSamples $taskInput) -PathType Leaf)) {
            throw "Required existing Vector sample is missing: $taskInput ($taskSamples)"
        }
    }
}
if (-not $OutputDirectory) {
    $taskSuffix = (Get-Date -Format 'yyyy-MM-dd_HH-mm-ss') + '_' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
    $OutputDirectory = Join-Path $taskRoot "artifacts/asc-dbc-cdd_$taskSuffix"
}
$taskOutput = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $taskOutput -Force | Out-Null
foreach ($taskCase in $taskCases) {
    $taskArguments = @(
        'kwp', (Join-Path $taskSamples "asc/Logging/$($taskCase.Asc).asc"),
        '--dbc', "$($taskCase.Channel)=$(Join-Path $taskSamples "dbc/CANdb/$($taskCase.Dbc).dbc")",
        '--cdd', (Join-Path $taskSamples "cdd/CDD/$($taskCase.Cdd).cdd"),
        '--ecu', 'Any_ECU_example', '--variant', 'COMMON_DIAGNOSTICS', '--allow-experimental',
        '--routes', (Join-Path $PSScriptRoot "$($taskCase.Name).routes.json"),
        '--policy', (Join-Path $PSScriptRoot "$($taskCase.Name).policy.json"),
        '-o', (Join-Path $taskOutput "$($taskCase.Name).jsonl"),
        '--report', (Join-Path $taskOutput "$($taskCase.Name).report.json")
    )
    Write-Host "Analyzing $($taskCase.Asc).asc + $($taskCase.Dbc).dbc + $($taskCase.Cdd).cdd"
    & $taskExe @taskArguments
    $taskExit = $LASTEXITCODE
    if ($taskExit -notin @(0, 3)) { throw "canlog failed: exit=$taskExit" }
    Write-Host "Saved $($taskCase.Name).jsonl and $($taskCase.Name).report.json (exit=$taskExit)"
}
Write-Host "Results: $taskOutput"
