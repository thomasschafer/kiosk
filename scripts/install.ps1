# Installs kiosk for the Herdr app on Windows: the kiosk mod and herdr-app-kiosk.exe,
# together in <app home>\mods\kiosk, where the mod looks for them.
#
#   irm https://raw.githubusercontent.com/thomasschafer/herdr-app-kiosk/main/scripts/install.ps1 | iex
#   scripts\install.ps1 [-Version <tag>] [-FromSource]
#
# It downloads a release's mod and prebuilt binary (the latest, or -Version, or
# $env:KIOSK_INSTALL_VERSION) and checks both against the release's SHA256SUMS, and, when
# the GitHub CLI is installed, against the release's build attestations. Run from a checkout
# with -FromSource, it installs that checkout's mod with a binary built from it. The app
# home is $env:HERDR_APP_HOME, else $env:XDG_CONFIG_HOME\herdr-app, else
# ~\.config\herdr-app, as the app has it.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repository = "thomasschafer/herdr-app-kiosk"
$releases = if ($env:KIOSK_RELEASES) { $env:KIOSK_RELEASES.TrimEnd("/") } else { "https://github.com/$repository/releases" }
$modName = "kiosk"
$target = "x86_64-pc-windows-msvc"

function Write-Step([string] $Message) {
    [Console]::Error.WriteLine("install: $Message")
}

function Stop-Install([string] $Message) {
    throw "install: $Message"
}

# Read from $args rather than a param block, which `irm ... | iex` would refuse.
$fromSource = $false
$version = $env:KIOSK_INSTALL_VERSION
for ($i = 0; $i -lt $args.Count; $i++) {
    switch ($args[$i]) {
        "-FromSource" { $fromSource = $true }
        "-Version" {
            if ($i + 1 -ge $args.Count) {
                Stop-Install "-Version needs a tag, such as v0.1.0"
            }
            $i++
            $version = $args[$i]
        }
        default { Stop-Install "unknown option $($args[$i])" }
    }
}

if ($env:HERDR_APP_HOME) {
    $appHome = $env:HERDR_APP_HOME
}
elseif ($env:XDG_CONFIG_HOME) {
    $appHome = Join-Path $env:XDG_CONFIG_HOME "herdr-app"
}
else {
    $appHome = Join-Path (Join-Path $HOME ".config") "herdr-app"
}
$modsDir = Join-Path $appHome "mods"
$modDir = Join-Path $modsDir $modName

$checkout = $null
if ($PSScriptRoot) {
    $candidate = Split-Path -Parent $PSScriptRoot
    if (
        (Test-Path -LiteralPath (Join-Path $candidate "Cargo.toml")) -and
        (Test-Path -LiteralPath (Join-Path (Join-Path (Join-Path $candidate "mods") $modName) "main.js"))
    ) {
        $checkout = $candidate
    }
}
if ($fromSource -and -not $checkout) {
    Stop-Install "-FromSource needs a checkout: clone the repository and run scripts\install.ps1 from it"
}

# A linked checkout is a development install: writing into it would put the binary in the
# checkout. It keeps working as it is.
if (Test-Path -LiteralPath $modDir) {
    $existing = Get-Item -LiteralPath $modDir -Force
    if ($existing.LinkType) {
        Stop-Install "$modDir is a link (to $($existing.Target)); remove it to install a copy instead"
    }
}

function Save-Download([string] $Url, [string] $Destination) {
    $uri = [Uri] $Url
    if ($uri.Scheme -eq [Uri]::UriSchemeFile) {
        Copy-Item -LiteralPath $uri.LocalPath -Destination $Destination -Force
        return
    }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    (New-Object Net.WebClient).DownloadFile($Url, $Destination)
}

# The tag the latest release's page redirects to.
function Get-LatestVersion {
    $request = [Net.HttpWebRequest]::Create("$releases/latest")
    $request.Method = "HEAD"
    $request.AllowAutoRedirect = $false
    $response = $request.GetResponse()
    try {
        $location = $response.Headers["Location"]
    }
    finally {
        $response.Close()
    }
    if (-not $location) {
        Stop-Install "could not find the latest release at $releases"
    }
    return ($location -split "/")[-1]
}

# Checks a downloaded release file against SHA256SUMS and, with the GitHub CLI, the
# release's build attestation.
function Confirm-Download([string] $Path, [string] $Sums) {
    $name = Split-Path -Leaf $Path
    $expected = $null
    foreach ($line in (Get-Content -LiteralPath $Sums)) {
        if ($line -match '^\s*([0-9A-Fa-f]{64})\s+\*?(\S+)\s*$' -and $Matches[2] -ceq $name) {
            $expected = $Matches[1]
        }
    }
    if (-not $expected) {
        Stop-Install "SHA256SUMS has no entry for $name"
    }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    if (-not $expected.Equals($actual, [StringComparison]::OrdinalIgnoreCase)) {
        Stop-Install "$name does not match its checksum"
    }
    if ($env:KIOSK_SKIP_ATTESTATION) {
        Write-Step "verified ${name}: checksum (KIOSK_SKIP_ATTESTATION skips its build attestation)"
    }
    elseif (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        Write-Step "verified ${name}: checksum (install the GitHub CLI to check its build attestation too)"
    }
    else {
        gh auth status *> $null
        if ($LASTEXITCODE -ne 0) {
            Write-Step "verified ${name}: checksum (log in to the GitHub CLI to check its build attestation too)"
            return
        }
        gh attestation verify $Path --repo $repository | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Stop-Install "$name has no valid build attestation from $repository"
        }
        Write-Step "verified ${name}: checksum and build attestation"
    }
}

$temporaryDirectory = Join-Path ([IO.Path]::GetTempPath()) "herdr-app-kiosk-install-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
try {
    $staged = Join-Path $temporaryDirectory $modName
    $stagedBin = Join-Path $staged "bin"
    if ($fromSource) {
        Write-Step "building $checkout"
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            Stop-Install "cargo is needed to build from source: https://rustup.rs/"
        }
        Push-Location $checkout
        try {
            cargo build --locked --release
            if ($LASTEXITCODE -ne 0) {
                Stop-Install "the build failed"
            }
        }
        finally {
            Pop-Location
        }
        New-Item -ItemType Directory -Path $stagedBin -Force | Out-Null
        $source = Join-Path (Join-Path $checkout "mods") $modName
        Copy-Item -LiteralPath (Join-Path $source "main.js"), (Join-Path $source "mod.json") -Destination $staged
        $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $checkout "target" }
        Copy-Item -LiteralPath (Join-Path (Join-Path $targetDir "release") "herdr-app-kiosk.exe") -Destination $stagedBin
    }
    else {
        if (-not $version) {
            $version = Get-LatestVersion
        }
        if ($version -notmatch '^v\d') {
            Stop-Install "not a release tag: $version"
        }
        $binary = "herdr-app-kiosk-$version-$target.exe"
        $mod = "herdr-app-kiosk-mod-$version.tar.gz"
        Write-Step "downloading herdr-app-kiosk $version"
        foreach ($file in "SHA256SUMS", $binary, $mod) {
            try {
                Save-Download "$releases/download/$version/$file" (Join-Path $temporaryDirectory $file)
            }
            catch {
                Stop-Install "could not download $file from release ${version}: $($_.Exception.Message)"
            }
        }
        $sums = Join-Path $temporaryDirectory "SHA256SUMS"
        Confirm-Download (Join-Path $temporaryDirectory $binary) $sums
        Confirm-Download (Join-Path $temporaryDirectory $mod) $sums
        tar -xzf (Join-Path $temporaryDirectory $mod) -C $temporaryDirectory
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath (Join-Path $staged "main.js"))) {
            Stop-Install "$mod does not hold the $modName mod"
        }
        New-Item -ItemType Directory -Path $stagedBin -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $temporaryDirectory $binary) -Destination (Join-Path $stagedBin "herdr-app-kiosk.exe")
    }

    # Swapped in whole, so the app never loads half of an update.
    New-Item -ItemType Directory -Path $modsDir -Force | Out-Null
    $old = "$modDir.old"
    if (Test-Path -LiteralPath $old) {
        Remove-Item -LiteralPath $old -Recurse -Force
    }
    if (Test-Path -LiteralPath $modDir) {
        Move-Item -LiteralPath $modDir -Destination $old
    }
    Move-Item -LiteralPath $staged -Destination $modDir
    if (Test-Path -LiteralPath $old) {
        Remove-Item -LiteralPath $old -Recurse -Force
    }
    Write-Step "installed $modDir"

    if (Get-Command herdr-app -ErrorAction SilentlyContinue) {
        $savedHome = $env:HERDR_APP_HOME
        try {
            $env:HERDR_APP_HOME = $appHome
            herdr-app check $modDir | ForEach-Object { [Console]::Error.WriteLine($_) }
            if ($LASTEXITCODE -ne 0) {
                Stop-Install "the app could not load $modDir"
            }
        }
        finally {
            $env:HERDR_APP_HOME = $savedHome
        }
    }
    Write-Step "bind a key to open it in $(Join-Path $appHome 'keys.json'), for example:"
    [Console]::Error.WriteLine('  { "global": { "ctrl-shift-r": "mod.open:kiosk" } }')
}
finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}
