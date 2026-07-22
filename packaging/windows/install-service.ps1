param(
    [Parameter(Mandatory = $true)]
    [string] $InstallDir
)

$ErrorActionPreference = 'Stop'
$serviceName = 'NeutrasearchHelper'
$executable = Join-Path $InstallDir 'neutrasearch-helper.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Missing service executable: $executable"
}

$resolvedInstallDir = (Resolve-Path -LiteralPath $InstallDir).Path.TrimEnd('\\')
$programFilesRoots = @($env:ProgramFiles, ${env:ProgramFiles(x86)}) |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
    ForEach-Object { [IO.Path]::GetFullPath($_).TrimEnd('\\') }
if (-not ($programFilesRoots | Where-Object {
    $resolvedInstallDir.StartsWith($_ + '\\', [StringComparison]::OrdinalIgnoreCase)
})) {
    throw "The privileged scanner must be installed beneath Program Files: $resolvedInstallDir"
}

$current = Get-Item -LiteralPath $resolvedInstallDir -Force
while ($null -ne $current) {
    if (($current.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing a reparse point in the privileged scanner path: $($current.FullName)"
    }
    if ($programFilesRoots -contains $current.FullName.TrimEnd('\\')) { break }
    $current = $current.Parent
}

# Reset away every pre-existing explicit ACE, then apply a deterministic
# machine-wide ACL. SID syntax avoids localized group names.
& "$env:SystemRoot\System32\icacls.exe" $resolvedInstallDir /reset /T /C /Q | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Could not reset scanner installation ACLs (icacls.exe exit $LASTEXITCODE)"
}
& "$env:SystemRoot\System32\icacls.exe" $resolvedInstallDir /inheritance:r `
    /grant:r '*S-1-5-18:(OI)(CI)(F)' '*S-1-5-32-544:(OI)(CI)(F)' '*S-1-5-32-545:(OI)(CI)(RX)' /T /C /Q | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Could not harden scanner installation ACLs (icacls.exe exit $LASTEXITCODE)"
}

$binaryPath = '"{0}" --windows-service' -f $executable
$existing = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -ne $existing) {
    if ($existing.Status -ne 'Stopped') {
        Stop-Service -Name $serviceName -Force
        $existing.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(15))
    }
    & "$env:SystemRoot\System32\sc.exe" config $serviceName start= auto binPath= $binaryPath DisplayName= 'Neutrasearch privileged scanner' | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Could not update $serviceName service (sc.exe exit $LASTEXITCODE)"
    }
    & "$env:SystemRoot\System32\sc.exe" description $serviceName 'Local-only native NTFS metadata scanner for Neutrasearch.' | Out-Null
} else {
    New-Service `
        -Name $serviceName `
        -BinaryPathName $binaryPath `
        -DisplayName 'Neutrasearch privileged scanner' `
        -Description 'Local-only native NTFS metadata scanner for Neutrasearch.' `
        -StartupType Automatic | Out-Null
}

# Restart after transient crashes, but never spin indefinitely.
& "$env:SystemRoot\System32\sc.exe" failure $serviceName reset= 86400 actions= restart/5000/restart/15000/""/0 | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Could not configure $serviceName recovery (sc.exe exit $LASTEXITCODE)"
}
Start-Service -Name $serviceName
(Get-Service -Name $serviceName).WaitForStatus('Running', [TimeSpan]::FromSeconds(15))
