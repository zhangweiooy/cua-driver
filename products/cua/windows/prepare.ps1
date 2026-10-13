param([switch]$Offline, [string]$Python = 'python')
$ErrorActionPreference = 'Stop'
$Arguments = @((Join-Path $PSScriptRoot '../package.py'),'prepare','--target','x86_64-pc-windows-msvc')
if ($Offline) { $Arguments += '--offline' }
& $Python @Arguments
exit $LASTEXITCODE
