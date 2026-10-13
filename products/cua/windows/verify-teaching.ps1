param(
    [Parameter(Mandatory=$true)][string]$Bundle,
    [Parameter(Mandatory=$true)][string]$Python,
    [switch]$AllowDesktop
)
$ErrorActionPreference = 'Stop'
if (-not $AllowDesktop) { throw 'Pass -AllowDesktop to open disposable windows and exercise native hooks.' }
$Root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$env:CUA_TEACHING_TEST_BUNDLE = (Resolve-Path -LiteralPath $Bundle).Path
$env:CUA_TEACHING_TEST_OUTPUT = Join-Path $PSScriptRoot '../.cache/tests'
Push-Location (Join-Path $Root 'libs/cua-driver/rust')
try {
    & cargo test --locked --release -p cua-driver-core --lib
    if ($LASTEXITCODE -ne 0) { throw 'Core regression failed' }
    & cargo test --locked --release -p platform-windows --lib teaching::tests::native_teaching -- --ignored --test-threads=1 --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Native teaching verification failed' }
} finally { Pop-Location }
& $Python (Join-Path $PSScriptRoot 'tests/test_teaching.py') $Bundle --allow-desktop
if ($LASTEXITCODE -ne 0) { throw 'MCP teaching acceptance failed' }
& $Python (Join-Path $PSScriptRoot 'tests/test_teaching_cli.py') $Bundle --allow-desktop
if ($LASTEXITCODE -ne 0) { throw 'Persistent CLI teaching acceptance failed' }
