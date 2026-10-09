# Installs Valkyrie on Windows: valk runs inside WSL2, and you use it from a
# Windows Terminal profile. From PowerShell, with the GitHub CLI logged in:
#   gh release download -R bjschnell/Valkyrie -p install.ps1 -O - | Out-String | iex
# or download install.ps1 from the release page and run:
#   powershell -ExecutionPolicy Bypass -File install.ps1
# It sets up WSL2 and Ubuntu if they're missing, installs valk inside it,
# offers Claude Code, and adds a "Valkyrie" Windows Terminal profile. Rerun it
# to update; a running daemon is handed to the new binary.
# VALK_WSL_DISTRO picks the distro (default: your default one, or Ubuntu),
# VALK_VERSION a release tag (default: the latest).
#
# Keep this file ASCII: piped through `Out-String | iex`, it's decoded with the
# console's code page. And no `exit`: under iex it would close the window.

& {
    $ErrorActionPreference = 'Stop'
    $repo = 'bjschnell/Valkyrie'
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
        Say "When $distro asks, choose a Linux username and password. If it then"
        Say "leaves you at a Linux prompt, type 'exit' to carry on here."
        & wsl.exe --install -d $distro
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
    if ((Wsl -d $distro --exec id -u) -eq '0') {
        Fail "$distro has no Linux user yet. Open '$distro' from the Start menu once to create one, then run this again."
        return
    }
    Say "using WSL distro $distro"

    # 2. valk inside it. A GitHub login on the Windows side is reused, so you
    # sign in once; WSLENV carries the token and version across.
    $token = $null
    if (Get-Command gh.exe -ErrorAction SilentlyContinue) {
        $ErrorActionPreference = 'Continue'
        $token = (& gh.exe auth token 2>$null | Out-String).Trim()
        $ErrorActionPreference = 'Stop'
    }
    $env:VALK_GH_TOKEN = $token
    $wslenv = $env:WSLENV # put back afterwards: under iex this is the user's session
    $env:WSLENV = (@('VALK_GH_TOKEN/u', 'VALK_VERSION/u', $env:WSLENV) | Where-Object { $_ }) -join ':'

    $setup = @'
set -euo pipefail
say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need=()
command -v curl >/dev/null || need+=(curl)
command -v gh >/dev/null || need+=(gh)
command -v paplay >/dev/null || need+=(pulseaudio-utils) # pings, through WSLg
if [ ${#need[@]} -gt 0 ]; then
    command -v apt-get >/dev/null || die "install these first: ${need[*]}"
    say "installing ${need[*]}; sudo asks for your Linux password"
    sudo apt-get update -qq
    sudo apt-get install -y -qq "${need[@]}" >/dev/null
fi

if ! gh auth status >/dev/null 2>&1; then
    if [ -n "${VALK_GH_TOKEN:-}" ]; then
        say "signing gh in with your Windows GitHub login"
        printf '%s\n' "$VALK_GH_TOKEN" | gh auth login --with-token
    else
        say "sign in to GitHub: the repo is private"
        gh auth login --hostname github.com --git-protocol https --web
    fi
fi

# Ubuntu's ~/.profile adds ~/.local/bin only if it existed at login.
export PATH="$HOME/.local/bin:$PATH"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
gh release download ${VALK_VERSION:+"$VALK_VERSION"} -R bjschnell/Valkyrie \
    -p install-release.sh -D "$tmp"
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
    # that sudo and gh's login prompt read from. Not in %TEMP%, which can be
    # an 8.3 short path that /mnt/c doesn't resolve.
    $file = Join-Path $env:LOCALAPPDATA 'valk-install.sh'
    [IO.File]::WriteAllText($file, ($setup -replace "`r`n", "`n"), (New-Object Text.UTF8Encoding $false))
    $linuxFile = Wsl -d $distro --exec wslpath -u $file
    & wsl.exe -d $distro --cd '~' --exec bash -l $linuxFile
    $ok = $LASTEXITCODE -eq 0
    Remove-Item $file -ErrorAction SilentlyContinue
    $env:VALK_GH_TOKEN = $null
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

    Say 'done. Open Windows Terminal and pick the Valkyrie profile.'
    if (Get-Command wt.exe -ErrorAction SilentlyContinue) {
        if (Ask 'Open it now?') { & wt.exe -p Valkyrie }
    } else {
        Warn 'Windows Terminal is not installed: winget install Microsoft.WindowsTerminal'
    }
}
