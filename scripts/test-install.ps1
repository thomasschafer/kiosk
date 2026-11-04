# Tests install.ps1 against a fake release, served the way GitHub serves releases, and a
# fake app.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) {
        throw "FAIL: $Message"
    }
}

function Assert-Same([string] $Left, [string] $Right) {
    Assert-True ((Get-FileHash -LiteralPath $Left).Hash -eq (Get-FileHash -LiteralPath $Right).Hash) "$Left differs from $Right"
}

# Text is compared ignoring line endings, as git on Windows checks files out with CRLF.
function Assert-SameText([string] $Left, [string] $Right) {
    $read = { param($Path) (Get-Content -LiteralPath $Path -Raw) -replace "`r`n", "`n" }
    Assert-True ((& $read $Left) -ceq (& $read $Right)) "$Left differs from $Right"
}

# A file:// URL for a local path, which [Uri] only builds from Windows paths.
function ConvertTo-FileUrl([string] $Path) {
    $full = [IO.Path]::GetFullPath($Path).Replace("\", "/")
    if (-not $full.StartsWith("/")) {
        $full = "/$full"
    }
    return "file://$full"
}

$windows = $IsWindows -or $env:OS -eq "Windows_NT"

# A program on PATH that runs `body`: a .cmd on Windows, a shell script elsewhere.
function New-FakeProgram([string] $Dir, [string] $Name, [string] $WindowsBody, [string] $UnixBody) {
    if ($windows) {
        Set-Content -LiteralPath (Join-Path $Dir "$Name.cmd") -Value "@echo off`r`n$WindowsBody"
    }
    else {
        $path = Join-Path $Dir $Name
        Set-Content -LiteralPath $path -Value "#!/bin/sh`n$UnixBody"
        chmod +x $path
    }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$projectMod = Join-Path (Join-Path $projectRoot "mods") "kiosk"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "herdr-app-kiosk-install-test-$([Guid]::NewGuid().ToString('N'))"
$bin = Join-Path $testRoot "bin"
$calls = Join-Path $testRoot "herdr-app-calls"
$ghCalls = Join-Path $testRoot "gh-calls"
$saved = @{}
foreach ($name in "HERDR_APP_HOME", "XDG_CONFIG_HOME", "KIOSK_RELEASES", "KIOSK_SKIP_ATTESTATION", "FAKE_GH_REFUSES", "PATH") {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name)
}
$server = $null

# Runs the installer and returns its exit code and output, unwrapped: PowerShell colours and
# boxes an error and wraps it to the console's width.
function Invoke-Installer([string[]] $Arguments = @()) {
    $output = & pwsh -NoLogo -NoProfile -File (Join-Path $testRoot "install.ps1") @Arguments 2>&1 | Out-String
    $plain = $output -replace "\x1b\[[0-9;]*m", "" -replace "\s*\r?\n\s*(\|\s*)?", " "
    return @{ Code = $LASTEXITCODE; Output = $plain }
}

function Assert-Installed([string] $AppHome) {
    $mod = Join-Path (Join-Path $AppHome "mods") "kiosk"
    Assert-Same (Join-Path (Join-Path $mod "bin") "herdr-app-kiosk.exe") (Join-Path $download $binary)
    Assert-SameText (Join-Path $mod "main.js") (Join-Path $projectMod "main.js")
    Assert-SameText (Join-Path $mod "mod.json") (Join-Path $projectMod "mod.json")
    Assert-True ((Get-Content -LiteralPath $calls -Raw).Contains("check $mod")) "the app did not check $mod"
}

try {
    $version = $null
    foreach ($line in (Get-Content -LiteralPath (Join-Path $projectRoot "Cargo.toml"))) {
        if (-not $version -and $line -match '^version = "([^"]*)"$') {
            $version = "v$($Matches[1])"
        }
    }
    $releases = Join-Path $testRoot "releases"
    $download = Join-Path (Join-Path $releases "download") $version
    $modStage = Join-Path (Join-Path $testRoot "mod") "kiosk"
    New-Item -ItemType Directory -Path $download, $modStage, $bin -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "install.ps1") -Destination $testRoot

    $binary = "herdr-app-kiosk-$version-x86_64-pc-windows-msvc.exe"
    $mod = "herdr-app-kiosk-mod-$version.tar.gz"
    Set-Content -LiteralPath (Join-Path $download $binary) -Value "released"
    Copy-Item -LiteralPath (Join-Path $projectMod "main.js"), (Join-Path $projectMod "mod.json") -Destination $modStage
    tar -czf (Join-Path $download $mod) -C (Join-Path $testRoot "mod") kiosk
    $sums = foreach ($file in $binary, $mod) {
        "$((Get-FileHash -LiteralPath (Join-Path $download $file) -Algorithm SHA256).Hash.ToLowerInvariant())  $file"
    }
    Set-Content -LiteralPath (Join-Path $download "SHA256SUMS") -Value $sums

    New-FakeProgram $bin "herdr-app" "echo %HERDR_APP_HOME% %*>>`"$calls`"" "printf '%s %s\n' `"`$HERDR_APP_HOME`" `"`$*`" >>'$calls'"
    New-Item -ItemType File -Path $calls | Out-Null
    $env:PATH = "$bin$([IO.Path]::PathSeparator)$($env:PATH)"
    $env:KIOSK_SKIP_ATTESTATION = "1"

    # GitHub answers /releases/latest with a redirect to the latest tag; this does the same.
    $python = if (Get-Command python3 -ErrorAction SilentlyContinue) { "python3" } else { "python" }
    $portFile = Join-Path $testRoot "port"
    $serverScript = Join-Path $testRoot "server.py"
    Set-Content -LiteralPath $serverScript -Value @'
import functools, http.server, sys
root, version, port_file = sys.argv[1:]
class Handler(http.server.SimpleHTTPRequestHandler):
    def do_HEAD(self):
        if self.path == "/latest":
            self.send_response(302)
            self.send_header("Location", f"/tag/{version}")
            self.end_headers()
            return
        super().do_HEAD()
    def log_message(self, *args):
        pass
server = http.server.HTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=root))
open(port_file, "w").write(str(server.server_port))
server.serve_forever()
'@
    $server = Start-Process -FilePath $python -ArgumentList @($serverScript, $releases, $version, $portFile) -PassThru -NoNewWindow
    for ($i = 0; $i -lt 100 -and -not (Test-Path -LiteralPath $portFile); $i++) {
        Start-Sleep -Milliseconds 50
    }
    Start-Sleep -Milliseconds 100
    Assert-True (Test-Path -LiteralPath $portFile) "the fake release server did not start"
    $env:KIOSK_RELEASES = "http://127.0.0.1:$((Get-Content -LiteralPath $portFile -Raw).Trim())"

    Write-Output "test: installs the latest release, verified"
    $home1 = Join-Path $testRoot "home"
    $env:HERDR_APP_HOME = $home1
    $run = Invoke-Installer
    Assert-True ($run.Code -eq 0) "the install failed: $($run.Output)"
    Assert-True ($run.Output.Contains("downloading herdr-app-kiosk $version")) "did not find ${version}: $($run.Output)"
    Assert-True ($run.Output.Contains("verified ${binary}: checksum")) "the binary was not verified: $($run.Output)"
    Assert-Installed $home1

    Write-Output "test: reinstalling a given release replaces the whole folder"
    $env:KIOSK_RELEASES = ConvertTo-FileUrl $releases
    $stale = Join-Path (Join-Path (Join-Path $home1 "mods") "kiosk") "stale.js"
    Set-Content -LiteralPath $stale -Value ""
    $run = Invoke-Installer @("-Version", $version)
    Assert-True ($run.Code -eq 0) "the reinstall failed: $($run.Output)"
    Assert-True (-not (Test-Path -LiteralPath $stale)) "a stale file survived"
    Assert-Installed $home1

    Write-Output "test: the app home follows XDG_CONFIG_HOME"
    $env:HERDR_APP_HOME = $null
    $env:XDG_CONFIG_HOME = Join-Path $testRoot "xdg"
    $run = Invoke-Installer @("-Version", $version)
    Assert-True ($run.Code -eq 0) "the install failed: $($run.Output)"
    Assert-Installed (Join-Path $env:XDG_CONFIG_HOME "herdr-app")
    $env:XDG_CONFIG_HOME = $null

    Write-Output "test: a file that does not match its checksum is refused"
    $tampered = Join-Path $testRoot "tampered"
    Copy-Item -LiteralPath $releases -Destination $tampered -Recurse
    Add-Content -LiteralPath (Join-Path (Join-Path (Join-Path $tampered "download") $version) $binary) -Value "tampered"
    $env:KIOSK_RELEASES = ConvertTo-FileUrl $tampered
    $env:HERDR_APP_HOME = Join-Path $testRoot "tampered-home"
    $run = Invoke-Installer @("-Version", $version)
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("does not match its checksum")) "installed a tampered binary: $($run.Output)"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path (Join-Path $env:HERDR_APP_HOME "mods") "kiosk"))) "installed something anyway"
    $env:KIOSK_RELEASES = ConvertTo-FileUrl $releases

    Write-Output "test: with the GitHub CLI, build attestations are checked too"
    $ghBin = Join-Path $testRoot "gh-bin"
    New-Item -ItemType Directory -Path $ghBin | Out-Null
    New-FakeProgram $ghBin "gh" "echo %*>>`"$ghCalls`"`r`nif defined FAKE_GH_REFUSES exit /b 1`r`nexit /b 0" "printf '%s\n' `"`$*`" >>'$ghCalls'`n[ -z `"`${FAKE_GH_REFUSES:-}`" ]"
    $pathWithoutGh = $env:PATH
    $env:PATH = "$ghBin$([IO.Path]::PathSeparator)$($env:PATH)"
    $env:KIOSK_SKIP_ATTESTATION = $null
    $env:HERDR_APP_HOME = Join-Path $testRoot "attested"
    $run = Invoke-Installer @("-Version", $version)
    Assert-True ($run.Code -eq 0) "the attested install failed: $($run.Output)"
    Assert-True ($run.Output.Contains("verified ${binary}: checksum and build attestation")) "no attestation check: $($run.Output)"
    Assert-True ((Get-Content -LiteralPath $ghCalls -Raw).Contains("--repo thomasschafer/herdr-app-kiosk")) "gh was not asked about the repository"
    Assert-Installed $env:HERDR_APP_HOME
    $env:FAKE_GH_REFUSES = "1"
    $env:HERDR_APP_HOME = Join-Path $testRoot "unattested"
    $run = Invoke-Installer @("-Version", $version)
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("has no valid build attestation")) "installed without a valid attestation: $($run.Output)"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path (Join-Path $env:HERDR_APP_HOME "mods") "kiosk"))) "installed something anyway"
    $env:FAKE_GH_REFUSES = $null
    $env:PATH = $pathWithoutGh
    $env:KIOSK_SKIP_ATTESTATION = "1"

    Write-Output "test: a linked development install is left alone"
    $linked = Join-Path $testRoot "linked"
    New-Item -ItemType Directory -Path (Join-Path $linked "mods") -Force | Out-Null
    $linkType = if ($windows) { "Junction" } else { "SymbolicLink" }
    New-Item -ItemType $linkType -Path (Join-Path (Join-Path $linked "mods") "kiosk") -Target $modStage | Out-Null
    $env:HERDR_APP_HOME = $linked
    $run = Invoke-Installer @("-Version", $version)
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("is a link")) "installed over a link: $($run.Output)"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $modStage "bin"))) "wrote into the linked folder"

    Write-Output "test: options are checked"
    $env:HERDR_APP_HOME = Join-Path $testRoot "options"
    $run = Invoke-Installer @("-Frobnicate")
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("unknown option -Frobnicate")) "took an unknown option: $($run.Output)"
    $run = Invoke-Installer @("-FromSource")
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("needs a checkout")) "built without a checkout: $($run.Output)"
    $run = Invoke-Installer @("-Version", "latest")
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("not a release tag")) "took a version that is not a tag: $($run.Output)"

    Write-Output "install.ps1 tests passed"
    # The last installer runs failed on purpose; their exit code is not this script's.
    $global:LASTEXITCODE = 0
}
finally {
    if ($server) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
    foreach ($name in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name])
    }
    Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
