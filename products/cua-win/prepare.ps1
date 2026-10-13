[CmdletBinding()]
param([switch]$Offline)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProductRoot = $PSScriptRoot
$RepoRoot = (Resolve-Path (Join-Path $ProductRoot '../..')).Path
$Lock = Get-Content (Join-Path $ProductRoot 'upstream.lock.json') -Raw | ConvertFrom-Json
$Cache = Join-Path $ProductRoot '.cache/upstream'
New-Item -ItemType Directory -Force -Path $Cache | Out-Null

function Assert-Hash([string]$Path, [string]$Expected) {
    $Actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Actual -ne $Expected) { throw "SHA256 mismatch: $Path (expected $Expected, got $Actual)" }
}

function Get-LockedAsset([string]$Tag, [string]$Name, [string]$Hash, [string]$Url = '') {
    $Destination = Join-Path $Cache $Name
    if (Test-Path -LiteralPath $Destination) {
        Assert-Hash $Destination $Hash
        return $Destination
    }
    if ($Offline) { throw "Missing locked build input in offline mode: $Destination" }
    $Partial = "$Destination.partial"
    if (-not $Url) { $Url = "https://github.com/trycua/cua/releases/download/$Tag/$Name" }
    & curl.exe --fail --location --retry 3 --connect-timeout 30 --max-time 900 --continue-at - --output $Partial $Url
    if ($LASTEXITCODE -ne 0) { throw "Locked asset download failed: $Url" }
    Assert-Hash $Partial $Hash
    Move-Item -LiteralPath $Partial -Destination $Destination
    return $Destination
}

foreach ($Component in @($Lock.driver, $Lock.perception)) {
    $Resolved = & git -C $RepoRoot rev-parse "$($Component.tag)^{commit}"
    if ($LASTEXITCODE -ne 0 -or "$Resolved".Trim() -ne $Component.commit) {
        throw "Release tag identity mismatch: $($Component.tag)"
    }
    $Archive = Join-Path $Cache "$($Component.tag)-source.tar"
    if (-not (Test-Path -LiteralPath $Archive)) {
        & git -C $RepoRoot archive --format=tar "--output=$Archive" $Component.tag
        if ($LASTEXITCODE -ne 0) { throw "Source export failed: $($Component.tag)" }
    }
    if ($Component.PSObject.Properties.Name -contains 'source_sha256') {
        Assert-Hash $Archive $Component.source_sha256
    } else {
        throw "Source digest has not been frozen for $($Component.tag)"
    }
    Get-LockedAsset $Component.tag $Component.asset $Component.sha256
}
Get-LockedAsset $Lock.perception.tag $Lock.perception.runtime_contract_asset $Lock.perception.runtime_contract_sha256
foreach ($Artifact in @($Lock.chinese_ocr.model, $Lock.chinese_ocr.dictionary)) {
    Get-LockedAsset '' $Artifact.filename $Artifact.sha256 $Artifact.url
}
Get-LockedAsset '' $Lock.msvc_runtime.filename $Lock.msvc_runtime.sha256 $Lock.msvc_runtime.url
Write-Host 'Release identities and downloaded inputs verified. No runtime was installed or started.'
