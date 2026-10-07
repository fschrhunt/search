#Requires -Version 5.1
# Exercise the Windows installer against a local release fixture, never GitHub.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Version
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Join-Path ([IO.Path]::GetTempPath()) ('search-installer-test-' + [Guid]::NewGuid())
$oldReleases = $env:SEARCH_RELEASES
$server = $null
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $package = Join-Path $root 'package'
    $release = Join-Path $root "download/$Version"
    New-Item -ItemType Directory -Path $package, $release -Force | Out-Null
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $package 'search.exe')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot '../LICENSE') -Destination $package
    $architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    $arch = if ($architecture -eq 'Arm64') { 'arm64' } else { 'amd64' }
    $archiveName = "search_${Version}_windows_${arch}.zip"
    $archive = Join-Path $release $archiveName
    Compress-Archive -Path "$package/search.exe", "$package/LICENSE" -DestinationPath $archive
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    $checksums = Join-Path $release 'checksums.txt'
    "$hash  $archiveName" | Set-Content -LiteralPath $checksums -Encoding ASCII

    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = $listener.LocalEndpoint.Port
    $listener.Stop()
    $server = Start-Process -FilePath 'python' -ArgumentList @('-m', 'http.server', "$port", '--bind', '127.0.0.1', '--directory', ('"' + $root + '"')) -PassThru -NoNewWindow -RedirectStandardOutput "$root/server.out" -RedirectStandardError "$root/server.err"
    $env:SEARCH_RELEASES = "http://127.0.0.1:$port"
    $ready = $false
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        try {
            Invoke-WebRequest -Uri "$env:SEARCH_RELEASES/download/$Version/checksums.txt" -UseBasicParsing -TimeoutSec 1 | Out-Null
            $ready = $true
            break
        } catch { Start-Sleep -Milliseconds 100 }
    }
    if (-not $ready) { throw 'Local installer fixture did not start' }

    $installer = Join-Path $PSScriptRoot '../install.ps1'
    $destination = Join-Path $root 'installed'
    & $installer -Version $Version -Dir $destination
    & $installer -Version $Version -Dir $destination
    if ((& "$destination/search.exe" version) -cne $Version -or $LASTEXITCODE -ne 0) {
        throw 'Install/update did not produce the expected executable'
    }
    $before = (Get-FileHash "$destination/search.exe").Hash
    ((('0' * 64) + '  ') + $archiveName) | Set-Content -LiteralPath $checksums -Encoding ASCII
    $rejected = $false
    try { & $installer -Version $Version -Dir $destination } catch {
        if ($_.Exception.Message -notmatch 'Checksum mismatch') { throw }
        $rejected = $true
    }
    if (-not $rejected -or (Get-FileHash "$destination/search.exe").Hash -ne $before) {
        throw 'Bad checksum was accepted or changed the existing executable'
    }
    Write-Host 'Windows installer: install, update, and checksum rejection passed'
} finally {
    $env:SEARCH_RELEASES = $oldReleases
    if ($server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force
        $server.WaitForExit()
    }
    Remove-Item -LiteralPath $root -Recurse -Force
}
