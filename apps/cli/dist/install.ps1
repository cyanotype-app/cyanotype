# Installs the `cyanotype` command line from your Cyanotype server (Windows, 64-bit).
#
#   irm https://<your server>/cli/install.ps1 | iex
#
# Installs to %LOCALAPPDATA%\Programs\cyanotype and adds that folder to your user PATH. Checks the checksum, and
# the signature when ssh-keygen (Windows' optional OpenSSH client) is available. The server fills in its own
# address below when it serves this file; set $env:CYANOTYPE_SERVER to override.
$ErrorActionPreference = 'Stop'

$ServerUrl = '{{ or (.Req.Header.Get "X-Forwarded-Proto") "http" }}://{{ .Host }}'
$PubKey = '@@PUBKEY@@'

function Fail($msg) { Write-Host "error: $msg" -ForegroundColor Red; throw $msg }

$server = if ($env:CYANOTYPE_SERVER) { $env:CYANOTYPE_SERVER } else { $ServerUrl }
if ($server -notmatch '^https?://[A-Za-z0-9.:_-]+$') {
    Fail 'do not know which server to download from: set $env:CYANOTYPE_SERVER = "https://your-server" and run again'
}
$server = $server.TrimEnd('/')

if (-not [Environment]::Is64BitOperatingSystem) { Fail 'only 64-bit Windows is supported' }
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -and -not $env:PROCESSOR_ARCHITEW6432) {
    Write-Host 'Note: this is an ARM PC; the x86_64 build runs under emulation.'
}
$target = 'x86_64-pc-windows-gnu'
$file = "cyanotype-$target.exe.gz"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("cyanotype-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Downloading cyanotype for $target from $server ..."
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$server/cli/SHA256SUMS" -OutFile "$tmp\SHA256SUMS"
        Invoke-WebRequest -UseBasicParsing -Uri "$server/cli/$file" -OutFile "$tmp\$file"
    } catch { Fail "could not download from $server/cli/ (does this server publish the CLI?): $($_.Exception.Message)" }

    $want = $null
    foreach ($line in Get-Content "$tmp\SHA256SUMS") {
        $parts = $line -split '\s+', 2
        if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq $file) { $want = $parts[0].ToLower() }
    }
    if (-not $want) { Fail "$file is not listed in SHA256SUMS" }
    $got = (Get-FileHash -Algorithm SHA256 "$tmp\$file").Hash.ToLower()
    if ($got -ne $want) { Fail "checksum mismatch for $file (expected $want, got $got); nothing was installed" }
    Write-Host 'Checksum OK.'

    $haveSig = $true
    try { Invoke-WebRequest -UseBasicParsing -Uri "$server/cli/SHA256SUMS.sig" -OutFile "$tmp\SHA256SUMS.sig" } catch { $haveSig = $false }
    if (-not $haveSig) {
        Write-Host "Note: this server's build is not signed. 'cyanotype update' will refuse to update it."
    } elseif (-not $PubKey.StartsWith('ssh-ed25519 ')) {
        Write-Host 'Signature not checked: this server did not publish a release key.'
    } elseif (Get-Command ssh-keygen -ErrorAction SilentlyContinue) {
        "release $PubKey" | Set-Content -Encoding ascii "$tmp\allowed"
        Get-Content -Raw "$tmp\SHA256SUMS" | & ssh-keygen -Y verify -f "$tmp\allowed" -I release -n cyanotype-cli -s "$tmp\SHA256SUMS.sig" *> $null
        if ($LASTEXITCODE -ne 0) { Fail 'the signature on SHA256SUMS does not verify; nothing was installed' }
        Write-Host 'Signature OK.'
    } else {
        Write-Host 'Signature not checked (ssh-keygen not found). The checksum came from the same server as the download.'
    }

    $dest = Join-Path $env:LOCALAPPDATA 'Programs\cyanotype'
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    $in = [IO.File]::OpenRead("$tmp\$file")
    try {
        $gz = New-Object IO.Compression.GzipStream($in, [IO.Compression.CompressionMode]::Decompress)
        $out = [IO.File]::Create("$tmp\cyanotype.exe")
        try { $gz.CopyTo($out) } finally { $out.Dispose(); $gz.Dispose() }
    } finally { $in.Dispose() }
    $exe = Join-Path $dest 'cyanotype.exe'
    if (Test-Path $exe) { Move-Item -Force $exe "$exe.old" -ErrorAction SilentlyContinue }
    Move-Item -Force "$tmp\cyanotype.exe" $exe
    Remove-Item -Force "$exe.old" -ErrorAction SilentlyContinue
    Write-Host "Installed $(& $exe --version) to $exe"

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $dest) {
        [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $dest), 'User')
        Write-Host "Added $dest to your PATH. Open a new terminal for it to take effect."
    }
    Write-Host ''
    Write-Host 'Next step:'
    Write-Host "  cyanotype login $server"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
