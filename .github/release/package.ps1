# Package and verify the same Windows zip in CI and before release publication.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Tag,
    [Parameter(Mandatory = $true)][string]$Arch,
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$work = Join-Path ([IO.Path]::GetTempPath()) ('search-package-' + [Guid]::NewGuid())
try {
    $package = Join-Path $work 'package'
    $verify = Join-Path $work 'verify'
    New-Item -ItemType Directory -Path $package, $OutputDirectory -Force | Out-Null
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $package 'search.exe')
    Copy-Item -LiteralPath LICENSE -Destination $package
    $archive = Join-Path $OutputDirectory "search_${Tag}_windows_${Arch}.zip"
    Compress-Archive -Path "$package/search.exe", "$package/LICENSE" -DestinationPath $archive
    Expand-Archive -LiteralPath $archive -DestinationPath $verify
    $members = @(Get-ChildItem -LiteralPath $verify -Recurse)
    if ($members.Count -ne 2 -or @(Compare-Object @('LICENSE', 'search.exe') @($members.Name)).Count -ne 0) {
        throw 'Unexpected Windows archive contents'
    }
    foreach ($file in @('search.exe', 'LICENSE')) {
        if ((Get-FileHash -LiteralPath "$package/$file").Hash -ne (Get-FileHash -LiteralPath "$verify/$file").Hash) {
            throw "Archive changed $file"
        }
    }
} finally {
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
