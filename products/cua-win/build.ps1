[CmdletBinding()]
param([switch]$Offline, [string]$ISCCPath = '', [switch]$Installer)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProductRoot = $PSScriptRoot
$RepoRoot = (Resolve-Path (Join-Path $ProductRoot '../..')).Path
$Lock = Get-Content (Join-Path $ProductRoot 'upstream.lock.json') -Raw | ConvertFrom-Json
$Cache = Join-Path $ProductRoot '.cache'
$Output = Join-Path $ProductRoot 'out'
$Standalone = Join-Path $Output "cua-win-$($Lock.product_version).dist"
$PerceptionSource = Join-Path $Cache 'perception-source'
$PerceptionPayload = Join-Path $Cache 'perception-payload'

function Invoke-Checked([string]$Executable, [string[]]$Arguments) {
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Executable failed with exit code $LASTEXITCODE" }
}

& (Join-Path $ProductRoot 'prepare.ps1') -Offline:$Offline
Invoke-Checked git @('-C', $RepoRoot, 'merge-base', '--is-ancestor', $Lock.driver.commit, 'HEAD')
if (Test-Path -LiteralPath $Standalone) {
    throw "Output already exists; preserve it and select a new product version or move it explicitly: $Standalone"
}
foreach ($Directory in @($PerceptionSource, $PerceptionPayload)) {
    New-Item -ItemType Directory -Force -Path $Directory | Out-Null
}
# Each component is built from its own release source and Cargo.lock.
Invoke-Checked tar @('-xf', (Join-Path $Cache "upstream/$($Lock.perception.tag)-source.tar"), '-C', $PerceptionSource)
Invoke-Checked tar @('-xzf', (Join-Path $Cache "upstream/$($Lock.perception.asset)"), '-C', $PerceptionPayload)
$DriverWorkspace = Join-Path $RepoRoot 'libs/cua-driver/rust'
$WorkerWorkspace = Join-Path $PerceptionSource 'libs/cua-driver/rust'
foreach ($Pair in @(@($DriverWorkspace, $Lock.driver), @($WorkerWorkspace, $Lock.perception))) {
    $LockHash = (Get-FileHash -LiteralPath (Join-Path $Pair[0] 'Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($LockHash -ne $Pair[1].cargo_lock_sha256) { throw "Cargo.lock differs from the selected release: $($Pair[0])" }
}
Push-Location $DriverWorkspace
try { Invoke-Checked cargo @('build', '--locked', '--release', '-p', 'cua-driver', '--bin', 'cua-driver', '--features', 'cua-win-product') }
finally { Pop-Location }
Push-Location $WorkerWorkspace
try { Invoke-Checked cargo @('build', '--locked', '--release', '-p', 'cua-perception', '--bin', 'cua-perception') }
finally { Pop-Location }
$Driver = Join-Path $DriverWorkspace 'target/release/cua-driver.exe'
$Worker = Join-Path $WorkerWorkspace 'target/release/cua-perception.exe'
foreach ($Binary in @($Driver, $Worker)) {
    if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) { throw "Missing Cargo output: $Binary" }
}
$Identity = & $Driver --version
if ($LASTEXITCODE -ne 0 -or $Identity -notlike "cua-win $($Lock.product_version) *") {
    throw 'Driver must be built with the cua-win-product feature and the locked product version'
}

New-Item -ItemType Directory -Force -Path (Join-Path $Standalone 'perception') | Out-Null
Copy-Item -LiteralPath $Driver -Destination (Join-Path $Standalone 'cua-win.exe')
Copy-Item -LiteralPath $Worker -Destination (Join-Path $Standalone 'perception/cua-perception.exe')
$RuntimeArchive = [IO.Compression.ZipFile]::OpenRead((Join-Path $Cache "upstream/$($Lock.msvc_runtime.filename)"))
try {
    foreach ($RuntimeFile in $Lock.msvc_runtime.files) {
        $Entry = $RuntimeArchive.GetEntry($Lock.msvc_runtime.member_prefix + $RuntimeFile.name)
        if ($null -eq $Entry) { throw "Missing locked CRT archive member: $($RuntimeFile.name)" }
        $Destination = Join-Path $Standalone "perception/$($RuntimeFile.name)"
        $InputStream = $Entry.Open()
        $OutputStream = [IO.File]::Create($Destination)
        try { $InputStream.CopyTo($OutputStream) }
        finally { $InputStream.Dispose(); $OutputStream.Dispose() }
        if ((Get-FileHash $Destination).Hash.ToLowerInvariant() -ne $RuntimeFile.sha256) {
            throw "CRT file digest mismatch: $($RuntimeFile.name)"
        }
    }
} finally { $RuntimeArchive.Dispose() }
foreach ($Directory in @('runtime', 'notices')) {
    Copy-Item -LiteralPath (Join-Path $PerceptionPayload $Directory) -Destination (Join-Path $Standalone 'perception') -Recurse
}
@'
This downstream development bundle distributes corresponding source separately.
See ../../SOURCE_OFFER.md and the adjacent build output source/ directory.
The source archive preserves the original upstream conversion materials.
'@ | Set-Content (Join-Path $Standalone 'perception/notices/SOURCE_OFFER.md') -Encoding utf8NoBOM
$ModelRoot = Join-Path $Standalone 'perception/models'
New-Item -ItemType Directory -Force -Path $ModelRoot | Out-Null
foreach ($Name in @('omniparser-icon-detect-1280-opset17.onnx', 'ppocrv5-mobile-det.onnx')) {
    Copy-Item -LiteralPath (Join-Path $PerceptionPayload "models/$Name") -Destination $ModelRoot
}
foreach ($Artifact in @($Lock.chinese_ocr.model, $Lock.chinese_ocr.dictionary)) {
    Copy-Item -LiteralPath (Join-Path $Cache "upstream/$($Artifact.filename)") -Destination $ModelRoot
}
$ModelManifest = Get-Content (Join-Path $PerceptionPayload 'model-manifest.json') -Raw | ConvertFrom-Json
$ModelManifest.identity.name = 'omniparser-v2-ppocrv5-zh-cua-win'
$ModelManifest.identity.version = $Lock.product_version
$ModelManifest.identity.source_revision = "detector:6600256cb0f1b07651e3bc86166196307bad7e2d;ocr-rec:$($Lock.chinese_ocr.revision)"
$ModelManifest.ocr.recognizer.model.path = "models/$($Lock.chinese_ocr.model.filename)"
$ModelManifest.ocr.recognizer.model.sha256 = $Lock.chinese_ocr.model.sha256
$ModelManifest.ocr.dictionary.path = "models/$($Lock.chinese_ocr.dictionary.filename)"
$ModelManifest.ocr.dictionary.sha256 = $Lock.chinese_ocr.dictionary.sha256
$ModelManifest | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $Standalone 'perception/model-manifest.json') -Encoding utf8NoBOM
$RuntimeContract = Get-Content (Join-Path $PerceptionPayload 'metadata/runtime-contract.json') -Raw | ConvertFrom-Json
$RuntimeContract.worker.sha256 = (Get-FileHash -LiteralPath $Worker -Algorithm SHA256).Hash.ToLowerInvariant()
$RuntimeContract.modelManifest.sha256 = (Get-FileHash (Join-Path $Standalone 'perception/model-manifest.json')).Hash.ToLowerInvariant()
($RuntimeContract.models | Where-Object role -eq 'ocr-recognize').name = $Lock.chinese_ocr.model.filename
($RuntimeContract.models | Where-Object role -eq 'ocr-recognize').sha256 = $Lock.chinese_ocr.model.sha256
$RuntimeContract.dictionary.name = $Lock.chinese_ocr.dictionary.filename
$RuntimeContract.dictionary.sha256 = $Lock.chinese_ocr.dictionary.sha256
$RuntimeContract | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $Standalone 'perception/runtime-contract.json') -Encoding utf8NoBOM
Copy-Item -LiteralPath (Join-Path $ProductRoot 'upstream.lock.json') -Destination $Standalone
Copy-Item -LiteralPath (Join-Path $RepoRoot 'LICENSE.md') -Destination (Join-Path $Standalone 'CUA-LICENSE.md')
Copy-Item -LiteralPath (Join-Path $ProductRoot 'SOURCE_OFFER.md') -Destination $Standalone
Copy-Item -LiteralPath (Join-Path $ProductRoot 'MODEL_SOURCES.md') -Destination $Standalone
Copy-Item -LiteralPath (Join-Path $ProductRoot 'TEACHING.md') -Destination $Standalone

$Files = [ordered]@{}
Get-ChildItem -LiteralPath $Standalone -File -Recurse | Sort-Object FullName | ForEach-Object {
    $Relative = $_.FullName.Substring($Standalone.Length + 1).Replace('\', '/')
    $Files[$Relative] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
}
$Manifest = [ordered]@{
    schema_version = 1; product = 'cua-win'; product_version = $Lock.product_version
    target = $Lock.target; minimum_windows_build = $Lock.minimum_windows_build
    driver = $Lock.driver; perception = $Lock.perception; protocol_version = $Lock.perception.protocol_version
    acceptance = @{ stage = 'P3-P5 development candidate'; chinese_ocr = 'candidate; full quality acceptance pending'; teaching = 'runtime-local exclusion, file output, CLI/MCP and native fixtures; full real-app acceptance pending'; windows_1903 = 'not_run' }
    files = $Files
}
$Manifest | ConvertTo-Json -Depth 14 | Set-Content (Join-Path $Standalone 'cua-win-package.json') -Encoding utf8NoBOM
Invoke-Checked (Join-Path $Standalone 'cua-win.exe') @('package-check')

# Keep model conversion materials and source outside the runtime/installer.
$SourceOutput = Join-Path $Output "source/cua-win-$($Lock.product_version)"
New-Item -ItemType Directory -Force -Path $SourceOutput | Out-Null
Copy-Item -LiteralPath (Join-Path $Cache "upstream/$($Lock.perception.asset)") -Destination $SourceOutput
Copy-Item -LiteralPath (Join-Path $Cache "upstream/$($Lock.driver.tag)-source.tar") -Destination $SourceOutput
Copy-Item -LiteralPath (Join-Path $Cache "upstream/$($Lock.perception.tag)-source.tar") -Destination $SourceOutput
Invoke-Checked git @('-C', $RepoRoot, 'diff', '--binary', "--output=$(Join-Path $SourceOutput 'cua-win.patch')", $Lock.driver.commit)
Copy-Item -LiteralPath (Join-Path $DriverWorkspace 'crates/cua-driver/src/product.rs') -Destination $SourceOutput
foreach ($Crate in @('cua-driver-core','platform-windows')) {
    $NewSource = Join-Path $SourceOutput "new-files/libs/cua-driver/rust/crates/$Crate/src"
    New-Item -ItemType Directory -Force -Path $NewSource | Out-Null
    Copy-Item -LiteralPath (Join-Path $DriverWorkspace "crates/$Crate/src/teaching.rs") -Destination $NewSource
}
Copy-Item -LiteralPath (Join-Path $DriverWorkspace 'crates/platform-windows/src/teaching_evidence.rs') -Destination (Join-Path $SourceOutput 'new-files/libs/cua-driver/rust/crates/platform-windows/src')
$ContractSource = Join-Path $SourceOutput 'new-files/libs/cua-driver/rust/crates/cua-driver-contract/src'
New-Item -ItemType Directory -Force -Path $ContractSource | Out-Null
Copy-Item -LiteralPath (Join-Path $DriverWorkspace 'crates/cua-driver-contract/src/teaching.rs') -Destination $ContractSource
foreach ($File in @('upstream.lock.json','prepare.ps1','build.ps1','cua-win.iss','SOURCE_OFFER.md','MODEL_SOURCES.md','README.md','TEACHING.md','teaching-verification.md','evidence-verification.md','convergence-verification.md','ime-verification.md','ime_fixture.py','test_package.py','test_teaching.py','test_teaching_cli.py','verify-teaching.ps1','evaluate_ocr.py')) {
    Copy-Item -LiteralPath (Join-Path $ProductRoot $File) -Destination $SourceOutput
}
$SourceTests = Join-Path $SourceOutput 'tests'
New-Item -ItemType Directory -Force -Path $SourceTests | Out-Null
Get-ChildItem -LiteralPath (Join-Path $ProductRoot 'tests') | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $SourceTests -Recurse -Force
}

if ($Installer) {
    if (-not $ISCCPath) { $ISCCPath = Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 6/ISCC.exe' }
    if (-not (Test-Path -LiteralPath $ISCCPath -PathType Leaf)) { throw 'Pass -ISCCPath with a supported Inno Setup compiler' }
    Invoke-Checked $ISCCPath @('/Q', "/DAppVersion=$($Lock.product_version)", "/DSourceDir=$Standalone", "/DOutputDir=$Output", (Join-Path $ProductRoot 'cua-win.iss'))
    $Setup = Join-Path $Output "cua-win-$($Lock.product_version)-win64-setup.exe"
    $Hash = (Get-FileHash -LiteralPath $Setup -Algorithm SHA256).Hash.ToLowerInvariant()
    "$Hash  $([IO.Path]::GetFileName($Setup))" | Set-Content (Join-Path $Output 'SHA256SUMS.txt') -Encoding ascii
}
Write-Host "Development bundle: $Standalone"
Write-Host 'Full Chinese OCR quality, real-app teaching and Windows 1903 acceptance are pending.'
