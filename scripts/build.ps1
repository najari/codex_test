param([switch]$Test, [switch]$Release)
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
    if ($Test) {
        & cargo test --locked
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
        & cargo clippy --locked --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    }
    if ($Release) { & cargo build --locked --release } else { & cargo build --locked }
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    if ($Release) {
        $distribution = Join-Path $taskRoot 'dist'
        New-Item -ItemType Directory -Path $distribution -Force | Out-Null
        $binary = if ($IsWindows -or $env:OS -eq 'Windows_NT') { 'canlog.exe' } else { 'canlog' }
        Copy-Item -LiteralPath (Join-Path $taskRoot "target\release\$binary") -Destination (Join-Path $distribution $binary) -Force
    }
} finally { Pop-Location }
