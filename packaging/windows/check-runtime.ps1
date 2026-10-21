param([Parameter(Mandatory = $true)][string] $BinaryDirectory)

$ErrorActionPreference = 'Stop'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$dumpbin = $null
if (Test-Path -LiteralPath $vswhere) {
    $dumpbin = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe' | Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($dumpbin)) {
        # Some runner images omit the component id vswhere filters on.
        $install = & $vswhere -latest -products '*' -property installationPath | Select-Object -First 1
        if (-not [string]::IsNullOrWhiteSpace($install)) {
            $dumpbin = Get-ChildItem -Path (Join-Path $install 'VC\Tools\MSVC') -Filter dumpbin.exe -Recurse -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match 'Hostx64\\x64' } |
                Sort-Object FullName -Descending |
                Select-Object -First 1 -ExpandProperty FullName
        }
    }
}
if ([string]::IsNullOrWhiteSpace($dumpbin)) {
    $command = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    if ($null -ne $command) { $dumpbin = $command.Source }
}
if ([string]::IsNullOrWhiteSpace($dumpbin)) {
    throw 'Could not locate dumpbin to verify Windows runtime dependencies'
}
$binaries = @(Get-ChildItem -LiteralPath $BinaryDirectory -Filter '*.exe' -File)
if ($binaries.Count -eq 0) { throw "No Windows binaries in $BinaryDirectory" }
foreach ($binary in $binaries) {
    $imports = & $dumpbin /DEPENDENTS $binary.FullName
    if ($LASTEXITCODE -ne 0) { throw "Could not inspect $($binary.Name)" }
    if ($imports -match '(?i)\b(?:vcruntime|msvcp|msvcr)\d+(?:_\w+)?\.dll\b') {
        throw "$($binary.Name) depends on a separately installed Visual C++ runtime"
    }
    Write-Output "$($binary.Name): no external Visual C++ runtime required"
}
