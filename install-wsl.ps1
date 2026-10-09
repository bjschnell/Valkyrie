# Installs Valkyrie on Windows: valk runs inside WSL2, and you use it from a
# Windows Terminal profile or a `valk` command. From PowerShell:
#   irm https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-wsl.ps1 | iex
# It sets up WSL2 and Ubuntu if they're missing, installs valk inside it,
# offers Claude Code, and adds a "Valkyrie" Windows Terminal profile. Rerun it
# to update; a running daemon is handed to the new binary.
# VALK_WSL_DISTRO picks the distro (default: your default one, or Ubuntu),
# VALK_VERSION a release tag (default: the latest).
#
# Keep this file ASCII, so no code page can garble it on the way into iex.
# And no `exit`: under iex it would close the window.

& {
    $ErrorActionPreference = 'Stop'
    $env:WSL_UTF8 = '1' # wsl.exe prints UTF-16 otherwise

    function Say($msg) { Write-Host '==> ' -ForegroundColor Magenta -NoNewline; Write-Host $msg }
    function Warn($msg) { Write-Host 'warning: ' -ForegroundColor Yellow -NoNewline; Write-Host $msg }
    function Fail($msg) { Write-Host 'error: ' -ForegroundColor Red -NoNewline; Write-Host $msg }
    function Ask($question) {
        $answer = Read-Host "$question [Y/n]"
        return ($answer -eq '' -or $answer -match '^[Yy]')
    }
    # Older wsl.exe ignores WSL_UTF8 and pads its output with NULs. Windows
    # PowerShell turns a native command's stderr into errors, which Stop throws.
    function Wsl {
        $ErrorActionPreference = 'Continue'
        (& wsl.exe @args 2>$null) | ForEach-Object { $_ -replace "`0", '' }
    }

    # Distros as name, version and whether it's the default, from `wsl -l -v`.
    function Get-Distros {
        $lines = Wsl --list --verbose
        if ($LASTEXITCODE -ne 0) { return @() }
        foreach ($line in $lines) {
            if ($line -match '^\s*(\*)?\s*(\S+)\s+\S+\s+(\d)\s*$') {
                [pscustomobject]@{ Name = $Matches[2]; Version = [int]$Matches[3]; Default = [bool]$Matches[1] }
            }
        }
    }

    # 1. A WSL2 distro.
    $distros = @(Get-Distros)
    $usable = @($distros | Where-Object { $_.Name -notlike 'docker-desktop*' })
    $distro = $env:VALK_WSL_DISTRO
    if (-not $distro) {
        $pick = $usable | Where-Object { $_.Default } | Select-Object -First 1
        if (-not $pick) { $pick = $usable | Where-Object { $_.Name -like 'Ubuntu*' } | Select-Object -First 1 }
        if ($pick) { $distro = $pick.Name } else { $distro = 'Ubuntu' }
    }
    if (-not ($distros | Where-Object { $_.Name -eq $distro })) {
        Say "installing WSL2 and $distro. Windows may ask for permission."
        # --no-launch: launching runs the distro's first-run setup, which ends in
        # a Linux shell that this script would sit waiting behind.
        & wsl.exe --install -d $distro --no-launch
        if (-not (Get-Distros | Where-Object { $_.Name -eq $distro })) {
            Say "restart Windows if it asked you to, then run this installer again."
            return
        }
    }
    $info = Get-Distros | Where-Object { $_.Name -eq $distro }
    if ($info.Version -ne 2) {
        Fail "$distro runs on WSL 1; valk needs WSL 2. Convert it with: wsl --set-version $distro 2"
        return
    }
    # The first-run setup didn't run (see --no-launch), so create the user
    # here. A new Ubuntu already starts as uid 1000 before that user exists,
    # so check for the account, not just for root.
    function Test-LinuxUser { [bool](Wsl -d $distro --exec sh -c 'u=$(id -u); test $u -ne 0 && getent passwd $u') }
    if (-not (Test-LinuxUser)) {
        Say "create your Linux user in $distro. It needn't match your Windows name;"
        Say "its password is what sudo asks for."
        $suggest = $env:USERNAME.ToLower() -replace '[^a-z0-9_-]', ''
        do {
            $name = Read-Host "Linux username [$suggest]"
            if (-not $name) { $name = $suggest }
        } until ($name -cmatch '^[a-z_][a-z0-9_-]{0,31}$')
        # uid 1000 is the one a new distro starts as; take another if it's used.
        & wsl.exe -d $distro -u root --exec sh -c 'if getent passwd 1000 >/dev/null; then useradd -m -s /bin/bash -G sudo $1; else useradd -m -u 1000 -s /bin/bash -G sudo $1; fi' sh $name
        for ($i = 0; $i -lt 3; $i++) {
            & wsl.exe -d $distro -u root --exec passwd $name
            if ($LASTEXITCODE -eq 0) { break }
        }
        # Older distros start as root: make this user the default.
        & wsl.exe -d $distro -u root --exec sh -c 'printf ''\n[user]\ndefault=%s\n'' $1 >> /etc/wsl.conf' sh $name
        & wsl.exe --terminate $distro | Out-Null
        if (-not (Test-LinuxUser)) {
            Fail "couldn't set up a Linux user in $distro (see above). Run this again to retry."
            return
        }
    }
    Say "using WSL distro $distro"

    # 2. valk inside it. WSLENV carries VALK_VERSION across.
    $wslenv = $env:WSLENV # put back afterwards: under iex this is the user's session
    $env:WSLENV = (@('VALK_VERSION/u', $env:WSLENV) | Where-Object { $_ }) -join ':'

    $setup = @'
set -euo pipefail
say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need=()
command -v curl >/dev/null || need+=(curl)
command -v paplay >/dev/null || need+=(pulseaudio-utils) # pings, through WSLg
if [ ${#need[@]} -gt 0 ]; then
    command -v apt-get >/dev/null || die "install these first: ${need[*]}"
    say "installing ${need[*]}; sudo asks for your Linux password"
    sudo apt-get update -qq
    sudo apt-get install -y -qq "${need[@]}" >/dev/null
fi

# Ubuntu's ~/.profile adds ~/.local/bin only if it existed at login.
export PATH="$HOME/.local/bin:$PATH"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if [ -n "${VALK_VERSION:-}" ]; then
    url="https://github.com/bjschnell/Valkyrie/releases/download/$VALK_VERSION/install-release.sh"
else
    url="https://github.com/bjschnell/Valkyrie/releases/latest/download/install-release.sh"
fi
curl -fsSL --retry 3 -o "$tmp/install-release.sh" "$url" || die "couldn't download $url"
bash "$tmp/install-release.sh"

if [ -t 0 ] && ! command -v claude >/dev/null; then
    printf 'Claude Code is not installed in WSL. Install it now? [Y/n] '
    read -r answer || answer=n
    case "$answer" in
        '' | [Yy]*) curl -fsSL https://claude.ai/install.sh | bash ;;
        *) say "skipped; Valkyrie only sees agents installed inside WSL" ;;
    esac
fi
'@
    # Run it from a file: piping a script in would take away the terminal
    # that sudo's password prompt reads from. Not in %TEMP%, which can be
    # an 8.3 short path that /mnt/c doesn't resolve.
    $file = Join-Path $env:LOCALAPPDATA 'valk-install.sh'
    [IO.File]::WriteAllText($file, ($setup -replace "`r`n", "`n"), (New-Object Text.UTF8Encoding $false))
    $linuxFile = Wsl -d $distro --exec wslpath -u $file
    & wsl.exe -d $distro --cd '~' --exec bash -l $linuxFile
    $ok = $LASTEXITCODE -eq 0
    Remove-Item $file -ErrorAction SilentlyContinue
    $env:WSLENV = $wslenv
    if (-not $ok) { Fail "installing valk inside $distro failed (see above)"; return }

    # 3. A Windows Terminal profile, as a fragment so settings.json is left alone.
    $fragments = Join-Path $env:LOCALAPPDATA 'Microsoft\Windows Terminal\Fragments\Valkyrie'
    New-Item -ItemType Directory -Force -Path $fragments | Out-Null
    $fragment = @{
        profiles = @(@{
            guid = '{6f1f7a0e-3c55-4a8e-9a43-7a6c6b8e5a11}'
            name = 'Valkyrie'
            # A login shell, so ~/.local/bin is on PATH for valk and its agents.
            commandline = "wsl.exe -d $distro --cd ~ --exec bash -lc valk"
            font = @{ face = 'Cascadia Mono' }
        })
    }
    [IO.File]::WriteAllText((Join-Path $fragments 'valkyrie.json'), ($fragment | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding $false))
    Say "added a 'Valkyrie' profile to Windows Terminal"

    # A `valk` command for Windows shells, which runs the one inside WSL.
    $binDir = Join-Path $env:LOCALAPPDATA 'Programs\Valkyrie'
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    $shim = Join-Path $binDir 'valk.cmd'
    $shimText = "@echo off`r`nrem Runs valk inside WSL; written by install.ps1.`r`n" +
        "wsl.exe -d $distro --cd ~ --exec bash -lc ""exec valk \""`$@\"""" valk %*`r`n"
    [IO.File]::WriteAllText($shim, $shimText, (New-Object Text.UTF8Encoding $false))
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (($userPath -split ';') -contains $binDir)) {
        [Environment]::SetEnvironmentVariable('Path', (@($userPath, $binDir) | Where-Object { $_ }) -join ';', 'User')
    }
    if (-not (($env:Path -split ';') -contains $binDir)) { $env:Path += ";$binDir" }
    Say "added a 'valk' command for PowerShell and cmd"

    # 4. Keep WSL running with no window open, so agents keep working.
    $wslconfig = Join-Path $env:USERPROFILE '.wslconfig'
    $config = ''
    if (Test-Path $wslconfig) { $config = [IO.File]::ReadAllText($wslconfig) }
    if ($config -notmatch '(?m)^[ \t]*vmIdleTimeout[ \t]*=[ \t]*-1[ \t]*\r?$') {
        Say "WSL stops a minute after its last window closes, and that stops your agents."
        if (Ask 'Keep WSL running in the background?') {
            if ($config -match '(?m)^[ \t]*vmIdleTimeout[ \t]*=') {
                $config = $config -replace '(?m)^[ \t]*vmIdleTimeout[ \t]*=[^\r\n]*', 'vmIdleTimeout=-1'
            } elseif ($config -match '(?m)^[ \t]*\[wsl2\][ \t]*\r?$') {
                $config = $config -replace '(?m)^[ \t]*\[wsl2\][ \t]*(?=\r?$)', "`$0`r`nvmIdleTimeout=-1"
            } else {
                if ($config -and -not $config.EndsWith("`n")) { $config += "`r`n" }
                $config += "[wsl2]`r`nvmIdleTimeout=-1`r`n"
            }
            [IO.File]::WriteAllText($wslconfig, $config, (New-Object Text.UTF8Encoding $false))
            Say "set in $wslconfig. It applies the next time WSL starts (or after 'wsl --shutdown')."
        }
    }

    Say "done. Type 'valk' in a new terminal, or pick the Valkyrie profile in Windows"
    Say 'Terminal. Terminals that were already open need a restart to see either.'
    if (Get-Command wt.exe -ErrorAction SilentlyContinue) {
        # Not `wt -p Valkyrie`: a running Terminal hasn't read the new profile,
        # and falls back to the default one.
        if (Ask 'Open it now?') { & wt.exe new-tab --title Valkyrie $shim }
    } else {
        Warn 'Windows Terminal is not installed: winget install Microsoft.WindowsTerminal'
    }
}
