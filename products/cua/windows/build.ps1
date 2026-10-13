[CmdletBinding()]
param([switch]$Offline, [switch]$Installer, [string]$Python = 'python', [string]$ISCCPath = '')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProductRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$RepoRoot = (Resolve-Path (Join-Path $ProductRoot '../..')).Path
$Lock = Get-Content (Join-Path $ProductRoot 'upstream.lock.json') -Raw | ConvertFrom-Json
$Target = 'x86_64-pc-windows-msvc'
$Config = $Lock.targets.$Target
$Cache = Join-Path $ProductRoot '.cache'
$Package = Join-Path $ProductRoot 'package.py'
$Output = Join-Path $ProductRoot 'out'
$Standalone = Join-Path $Output "cua-win-$($Lock.product_version).dist"
function Invoke-Checked([string]$Exe, [string[]]$Arguments) {
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Exe failed with exit code $LASTEXITCODE" }
}
$Prepare = @($Package, 'prepare', '--target', $Target)
if ($Offline) { $Prepare += '--offline' }
Invoke-Checked $Python $Prepare
if (Test-Path -LiteralPath $Standalone) { throw "Preserve existing output: $Standalone" }
$DriverWorkspace = Join-Path $RepoRoot 'libs/cua-driver/rust'
$WorkerWorkspace = Join-Path $Cache "$($Lock.perception.tag)/source/libs/cua-driver/rust"
$DriverTarget = Join-Path $Cache "$($Lock.driver.tag)/$Target/build"
$WorkerTarget = Join-Path $Cache "$($Lock.perception.tag)/$Target/build"
foreach ($Pair in @(@($DriverWorkspace, $Lock.driver), @($WorkerWorkspace, $Lock.perception))) {
    if ((Get-FileHash -LiteralPath (Join-Path $Pair[0] 'Cargo.lock')).Hash.ToLowerInvariant() -ne $Pair[1].cargo_lock_sha256) { throw 'Component Cargo.lock mismatch' }
}
Push-Location $DriverWorkspace
try { Invoke-Checked cargo @('build','--locked','--release','--target-dir',$DriverTarget,'-p','cua-driver','--bin','cua-driver','--features','cua-product') }
finally { Pop-Location }
Push-Location $WorkerWorkspace
try { Invoke-Checked cargo @('build','--locked','--release','--target-dir',$WorkerTarget,'-p','cua-perception','--bin','cua-perception') }
finally { Pop-Location }
Invoke-Checked $Python @($Package,'assemble','--target',$Target,'--driver',(Join-Path $DriverTarget 'release/cua-driver.exe'),'--worker',(Join-Path $WorkerTarget 'release/cua-perception.exe'),'--output',$Standalone)
Invoke-Checked (Join-Path $Standalone $Config.binary) @('package-check')
Invoke-Checked $Python @($Package,'sources','--target',$Target)
if ($Installer) {
    if (-not $ISCCPath) { $ISCCPath = Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 6/ISCC.exe' }
    Invoke-Checked $ISCCPath @('/Q',"/DAppVersion=$($Lock.product_version)","/DSourceDir=$Standalone","/DOutputDir=$Output",(Join-Path $PSScriptRoot 'cua-win.iss'))
    $Setup = Join-Path $Output "cua-win-$($Lock.product_version)-win64-setup.exe"
    "$((Get-FileHash -LiteralPath $Setup).Hash.ToLowerInvariant())  $([IO.Path]::GetFileName($Setup))" | Set-Content (Join-Path $Output 'SHA256SUMS.txt') -Encoding ascii
}
Write-Host "Development bundle: $Standalone"
