# Installs native Valkyrie for Windows x64. No WSL or administrator access needed.
#   irm https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install.ps1 | iex
# VALK_VERSION selects a release tag; VALK_BIN_DIR selects the install directory.
# Versioned binaries allow updates while the old daemon is still running.
& {
    $ErrorActionPreference = 'Stop'
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    function Say($msg) { Write-Host "==> $msg" -ForegroundColor Magenta }
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('valk-install-' + [Guid]::NewGuid())
    try {
        if (-not [Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') {
            throw 'This release supports Windows x64. Use install-wsl.ps1 on Windows ARM64.'
        }
        if ([Environment]::OSVersion.Version.Build -lt 17763) {
            throw 'Native Valkyrie needs Windows 10 version 1809 or newer (ConPTY).'
        }
        $version = $env:VALK_VERSION
        if (-not $version) {
            $release = Invoke-RestMethod 'https://api.github.com/repos/bjschnell/Valkyrie/releases/latest'
            $version = $release.tag_name
        }
        if ($version -notmatch '^v[0-9A-Za-z._-]+$') { throw 'VALK_VERSION must be a release tag such as v0.0.2.' }
        $binDir = $env:VALK_BIN_DIR
        if (-not $binDir) { $binDir = Join-Path $env:LOCALAPPDATA 'Programs\Valkyrie' }
        $binDir = [IO.Path]::GetFullPath($binDir)
        # The launcher uses cmd.exe, which expands these even within quotes.
        if ($binDir.IndexOfAny([char[]]'%"!') -ge 0) { throw 'Choose VALK_BIN_DIR without %, ! or double quotes.' }
        New-Item -ItemType Directory -Path $temp | Out-Null
        $name = 'valk-x86_64-pc-windows-msvc.zip'
        $url = "https://github.com/bjschnell/Valkyrie/releases/download/$version/$name"
        $archive = Join-Path $temp $name
        Say "downloading $version for Windows x64"
        Invoke-WebRequest -UseBasicParsing $url -OutFile $archive
        Invoke-WebRequest -UseBasicParsing "$url.sha256" -OutFile "$archive.sha256"
        $checksum = ([IO.File]::ReadAllText("$archive.sha256") -split '\s+')[0]
        if ($checksum -notmatch '^[0-9a-fA-F]{64}$' -or (Get-FileHash $archive -Algorithm SHA256).Hash -ne $checksum) {
            throw 'Release checksum does not match; nothing was installed.'
        }
        Expand-Archive $archive -DestinationPath (Join-Path $temp 'unpacked')
        $download = Join-Path $temp 'unpacked\valk.exe'
        if (-not (Test-Path $download)) { throw 'Release archive is missing valk.exe.' }
        # Always use a fresh directory, including when reinstalling the same tag.
        $relative = 'versions\' + $version + '-' + [Guid]::NewGuid().ToString('N')
        $installDir = Join-Path $binDir $relative
        New-Item -ItemType Directory -Force -Path $installDir | Out-Null
        $exe = Join-Path $installDir 'valk.exe'
        Copy-Item $download $exe
        & $exe --version
        if ($LASTEXITCODE -ne 0) { throw 'The downloaded binary could not run.' }
        # Windows PowerShell treats redirected native stderr as an error record.
        $ErrorActionPreference = 'Continue'
        $upgrade = & $exe upgrade --exe $exe 2>&1
        $upgradeCode = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        if ($upgradeCode -ne 0 -and ($upgrade | Out-String) -notmatch 'nothing to upgrade') {
            throw "Daemon upgrade failed. The previous launcher is unchanged: $upgrade"
        }
        Say ($upgrade | Out-String).Trim()
        $launcher = "@echo off`r`nsetlocal DisableDelayedExpansion`r`n" +
            """%~dp0$relative\valk.exe"" %*`r`nexit /b %errorlevel%`r`n"
        [IO.File]::WriteAllText((Join-Path $binDir 'valk.cmd'), $launcher, (New-Object Text.UTF8Encoding $false))
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (-not (($userPath -split ';') -contains $binDir)) {
            [Environment]::SetEnvironmentVariable('Path', (@($userPath, $binDir) | Where-Object { $_ }) -join ';', 'User')
        }
        if (-not (($env:Path -split ';') -contains $binDir)) { $env:Path += ";$binDir" }
        $fragments = Join-Path $env:LOCALAPPDATA 'Microsoft\Windows Terminal\Fragments\Valkyrie'
        New-Item -ItemType Directory -Force -Path $fragments | Out-Null
        $fragment = @{ profiles = @(@{
            guid = '{6f1f7a0e-3c55-4a8e-9a43-7a6c6b8e5a11}'
            name = 'Valkyrie'
            commandline = 'cmd.exe /d /c ""' + (Join-Path $binDir 'valk.cmd') + '""'
            startingDirectory = $env:USERPROFILE
            font = @{ face = 'Cascadia Mono' }
        }) }
        [IO.File]::WriteAllText((Join-Path $fragments 'valkyrie.json'), ($fragment | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding $false))
        Say "installed native valk in $binDir. Type valk to open it."
        Say 'Restart Windows Terminal to see its Valkyrie profile. Install agents on Windows and use your Windows repos.'
        Say 'Updates restart sessions: agent conversations resume; running shell commands stop. Old version directories are retained while in use.'
    } catch {
        Write-Host "error: $_" -ForegroundColor Red
    } finally {
        Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
    }
}
