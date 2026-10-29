$ErrorActionPreference = 'Stop'

if (-not [Environment]::Is64BitOperatingSystem -or
    $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64' -and $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw 'Ravel binary installer currently supports Windows x64 only.'
}

$asset = 'ravel-windows-x86_64.zip'
$url = "https://github.com/Sidearker/Ravel/releases/latest/download/$asset"
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Ravel'

try {
    New-Item -ItemType Directory -Path $tempDir | Out-Null
    $archive = Join-Path $tempDir $asset
    Invoke-WebRequest -Uri $url -OutFile $archive
    Expand-Archive -LiteralPath $archive -DestinationPath $tempDir

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $tempDir 'ravel.exe') -Destination (Join-Path $installDir 'ravel.exe') -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if ($pathEntries -notcontains $installDir) {
        [Environment]::SetEnvironmentVariable('Path', (($pathEntries + $installDir) -join ';'), 'User')
    }
    if (($env:Path -split ';') -notcontains $installDir) {
        $env:Path = "$installDir;$env:Path"
    }

    Write-Host "Installed Ravel to $installDir\ravel.exe"
    Write-Host 'Run `ravel` in a new terminal.'
}
finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
