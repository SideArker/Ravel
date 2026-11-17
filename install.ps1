$ErrorActionPreference = 'Stop'

if (-not [Environment]::Is64BitOperatingSystem -or
    $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64' -and $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw 'Ravel binary installer currently supports Windows x64 only.'
}

$asset = 'ravel-windows-x86_64.zip'
$url = "https://github.com/Sidearker/Ravel/releases/latest/download/$asset"
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Ravel'
$exePath = Join-Path $installDir 'ravel.exe'

try {
    New-Item -ItemType Directory -Path $tempDir | Out-Null
    $archive = Join-Path $tempDir $asset
    Invoke-WebRequest -Uri $url -OutFile $archive
    Expand-Archive -LiteralPath $archive -DestinationPath $tempDir

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $tempDir 'ravel.exe') -Destination $exePath -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if ($pathEntries -notcontains $installDir) {
        [Environment]::SetEnvironmentVariable('Path', (($pathEntries + $installDir) -join ';'), 'User')
    }
    if (($env:Path -split ';') -notcontains $installDir) {
        $env:Path = "$installDir;$env:Path"
    }

    $programsDir = [Environment]::GetFolderPath('Programs')
    if ($programsDir) {
        New-Item -ItemType Directory -Path $programsDir -Force | Out-Null
        $shortcutPath = Join-Path $programsDir 'Ravel.lnk'
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $exePath
        $shortcut.WorkingDirectory = $installDir
        $shortcut.Description = 'Share a mouse and keyboard over your LAN'
        $shortcut.Save()
    }

    Write-Host "Installed Ravel to $exePath"
    Write-Host 'Search for "Ravel" in the Start Menu or run `ravel` in a new terminal.'
}
finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
