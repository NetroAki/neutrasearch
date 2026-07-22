$ErrorActionPreference = 'Stop'
$serviceName = 'NeutrasearchHelper'
$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -eq $service) {
    exit 0
}
if ($service.Status -ne 'Stopped') {
    Stop-Service -Name $serviceName -Force
    $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(15))
}
& "$env:SystemRoot\System32\sc.exe" delete $serviceName | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Could not remove $serviceName service (sc.exe exit $LASTEXITCODE)"
}
