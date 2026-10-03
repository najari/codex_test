param([switch]$Test, [switch]$Release, [switch]$TestEngine, [string]$EngineSamples, [switch]$Cdd)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path $PSScriptRoot -Parent
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$env:PATH = "$cargoBin;$env:PATH"
if ($IsWindows -or $env:OS -eq 'Windows_NT') {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $vswhere) {
        $vsPath = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($vsPath) {
            $vcvars = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat'
            $environment = & $env:COMSPEC /d /c "call `"$vcvars`" >nul && set"
            if ($LASTEXITCODE -ne 0) { throw 'Visual C++ environment initialization failed' }
            foreach ($entry in $environment) {
                if ($entry -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process') }
            }
        }
    }
}
Push-Location $taskRoot
try {
    $taskFeatures = if ($Cdd) { @('--features', 'cdd') } else { @() }
    if ($TestEngine) {
        $taskMetadata = & cargo metadata --locked --format-version 1 | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed' }
        $taskEngine = $taskMetadata.packages | Where-Object name -eq 'candb-engine'
        if (-not $taskEngine) { throw 'Pinned candb-engine dependency not found' }
        $taskPinnedRoot = Split-Path (Split-Path $taskEngine.manifest_path -Parent) -Parent
        $taskEngineManifest = $taskEngine.manifest_path
        $taskEngineTarget = Join-Path $taskRoot 'target\dbc-engine'
        if ($EngineSamples) {
            # Test a copy of the pinned engine, with locally supplied corpus data.
            # Never edit the upstream checkout or original sample files.
            $taskSampleRoot = (Resolve-Path -LiteralPath $EngineSamples).Path
            $taskTestRoot = Join-Path $taskRoot 'target\dbc-engine-source'
            New-Item -ItemType Directory -Path $taskTestRoot -Force | Out-Null
            foreach ($taskName in @('Cargo.toml', 'Cargo.lock', 'engine', 'samples')) {
                Copy-Item -LiteralPath (Join-Path $taskPinnedRoot $taskName) -Destination $taskTestRoot -Recurse -Force
            }
            foreach ($taskFolder in @('dbc_files', 'cantools\dbc', 'model3')) {
                $taskDestination = Join-Path $taskTestRoot "samples\$taskFolder"
                New-Item -ItemType Directory -Path $taskDestination -Force | Out-Null
                Get-ChildItem -LiteralPath (Join-Path $taskSampleRoot $taskFolder) -File -Filter '*.dbc' | ForEach-Object {
                    Copy-Item -LiteralPath $_.FullName -Destination $taskDestination -Force
                }
            }
            $taskEngineManifest = Join-Path $taskTestRoot 'engine\Cargo.toml'
            # Corpus paths are embedded by env!(CARGO_MANIFEST_DIR); use a
            # separate target from tests compiled directly in Cargo's checkout.
            $taskEngineTarget = Join-Path $taskRoot 'target\dbc-engine-with-corpus'
        } elseif (-not (Test-Path -LiteralPath (Join-Path $taskPinnedRoot 'samples\dbc_files'))) {
            throw 'Upstream corpus is not shipped in Git. Use -EngineSamples <local engine samples directory> for -TestEngine.'
        }
        & cargo test --locked --manifest-path $taskEngineManifest --target-dir $taskEngineTarget
        if ($LASTEXITCODE -ne 0) { throw 'Pinned DBC engine tests failed' }
    }
    if ($Test) {
        & cargo test --locked @taskFeatures
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
        & cargo clippy --locked @taskFeatures --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    }
    if ($Release) { & cargo build --locked @taskFeatures --release } else { & cargo build --locked @taskFeatures }
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    if ($Release) {
        $distribution = Join-Path $taskRoot 'dist'
        New-Item -ItemType Directory -Path $distribution -Force | Out-Null
        $binary = if ($IsWindows -or $env:OS -eq 'Windows_NT') { 'canlog.exe' } else { 'canlog' }
        Copy-Item -LiteralPath (Join-Path $taskRoot "target\release\$binary") -Destination (Join-Path $distribution $binary) -Force
    }
} finally { Pop-Location }
