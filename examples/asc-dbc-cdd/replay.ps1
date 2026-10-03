param(
    [ValidateSet('comfort', 'engine')][string]$Sample = 'comfort',
    [double]$Speed = 2,
    [switch]$NoWait,
    [ValidateSet('console', 'jsonl')][string]$Sink = 'console',
    [string]$OutputDirectory,
    [string]$Exe
)
$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
if (-not $Exe) { $Exe = Join-Path $taskRoot 'dist/canlog.exe' }
$taskExe = (Resolve-Path -LiteralPath $Exe).Path
$taskSamples = Join-Path $taskRoot 'can_example/vector_samples/2026-10-03/groups/CANoe_13.0.172/CAN/CANSystemDemo'
$taskCase = if ($Sample -eq 'comfort') {
    @{ Asc = 'ComfortDiagData'; Dbc = 'Comfort'; Cdd = 'CANSystemDoor'; Channel = 1 }
} else {
    @{ Asc = 'EngineDiagData'; Dbc = 'PowerTrain'; Cdd = 'CANSystem'; Channel = 2 }
}
if (-not $OutputDirectory) {
    $taskSuffix = (Get-Date -Format 'yyyy-MM-dd_HH-mm-ss') + '_' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
    $OutputDirectory = Join-Path $taskRoot "artifacts/asc-dbc-cdd-replay_$taskSuffix"
}
$taskOutput = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $taskOutput -Force | Out-Null
$taskArguments = @(
    'replay', (Join-Path $taskSamples "asc/Logging/$($taskCase.Asc).asc"),
    '--dbc', "$($taskCase.Channel)=$(Join-Path $taskSamples "dbc/CANdb/$($taskCase.Dbc).dbc")",
    '--cdd', (Join-Path $taskSamples "cdd/CDD/$($taskCase.Cdd).cdd"),
    '--ecu', 'Any_ECU_example', '--variant', 'COMMON_DIAGNOSTICS', '--allow-experimental',
    '--protocol', 'kwp2000-vector',
    '--routes', (Join-Path $PSScriptRoot "$Sample.routes.json"),
    '--policy', (Join-Path $PSScriptRoot "$Sample.policy.json"),
    '--speed', $Speed.ToString([Globalization.CultureInfo]::InvariantCulture), '--sink', $Sink,
    '--report', (Join-Path $taskOutput "$Sample.replay.report.json")
)
if ($NoWait) { $taskArguments += '--no-wait' }
if ($Sink -eq 'jsonl') { $taskArguments += @('-o', (Join-Path $taskOutput "$Sample.replay.jsonl")) }
& $taskExe @taskArguments
$taskExit = $LASTEXITCODE
if ($taskExit -notin @(0, 3)) { throw "canlog replay failed: exit=$taskExit" }
Write-Host "Replay results: $taskOutput (exit=$taskExit)"
