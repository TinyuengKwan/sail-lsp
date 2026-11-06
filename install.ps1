# Native Windows bootstrap. Management is delegated to a temporary copy so a
# running installer never locks the installed executable during replacement.
$ErrorActionPreference = 'Stop'
$installerArgs = @($args | ForEach-Object { [string]$_ })
$version = 'latest'
$localData = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { [Environment]::GetFolderPath('LocalApplicationData') }
$prefix = if ($env:SAIL_LSP_HOME) { $env:SAIL_LSP_HOME } else { Join-Path $localData 'sail-lsp' }
$base = if ($env:SAIL_LSP_RELEASE_BASE_URL) { $env:SAIL_LSP_RELEASE_BASE_URL } else { 'https://github.com/TinyuengKwan/sail-lsp/releases' }
$archive = $null; $checksum = $null; $sourceDir = $null; $revision = $null
$fromSource = $false; $lifecycle = $false
$valueOptions = @('--version','--prefix','--archive','--sha256','--path','--rev','--release-base-url','--config-home','--editor-root','--editor','--vsix','--code-command','--profile')
for ($i=0; $i -lt $installerArgs.Count; $i++) {
    $key = $installerArgs[$i]
    if ($key -in @('--help','-h')) {
        Write-Output 'install.ps1 [--version TAG] [--prefix DIR] [--editor vscode|zed|neovim|helix|emacs]'
        Write-Output '  [--from-source --rev REF | --path CHECKOUT | --archive FILE --sha256 HASH]'
        Write-Output '  [--no-modify-path] [--list | --rollback [--version ID] | --uninstall]'
        return
    }
    if ($key -in @('--path','--rev','--from-source')) { $fromSource = $true }
    if ($key -in @('--uninstall','--rollback','--list')) { $lifecycle = $true }
    if ($key -in $valueOptions) {
        $i++; if ($i -ge $installerArgs.Count) { throw "$key requires a value" }
        $value = $installerArgs[$i]
        switch ($key) {
            '--version' { $version=$value }; '--prefix' { $prefix=$value }; '--archive' { $archive=$value }
            '--sha256' { $checksum=$value }; '--path' { $sourceDir=$value }; '--rev' { $revision=$value }
            '--release-base-url' { $base=$value.TrimEnd('/') }
        }
    }
}
if ($version -notmatch '^[a-zA-Z0-9][a-zA-Z0-9._-]*$') { throw 'Invalid version' }
if ($revision -and $revision.StartsWith('-')) { throw 'Invalid revision' }
$work = Join-Path ([IO.Path]::GetTempPath()) ('sail-lsp-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
function Invoke-Checked([string]$Program,[string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
function Get-Asset([string]$Url,[string]$Destination) {
    if ($Url.StartsWith('file://')) { Copy-Item -LiteralPath $Url.Substring(7) -Destination $Destination; return }
    if (-not $Url.StartsWith('https://')) { throw 'Release URLs must use HTTPS or an explicit file:// offline mirror' }
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Destination -TimeoutSec 300
}
try {
    if ($lifecycle) {
        $installed=Join-Path $prefix 'bin/sail-lsp.exe'
        if (-not (Test-Path -LiteralPath $installed)) { throw "No managed executable at $installed" }
        $runner=Join-Path $work 'sail-lsp.exe'; Copy-Item -LiteralPath $installed -Destination $runner
        Invoke-Checked $runner (@('install')+$installerArgs)
        return
    }
    if ($fromSource) {
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Source installation requires Rust, CMake and the MSVC C++ build tools' }
        if (-not $sourceDir) {
            if ($PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot 'crates/sail-lsp/Cargo.toml')) -and -not $revision -and $version -eq 'latest') { $sourceDir=$PSScriptRoot }
            else {
                $sourceDir=Join-Path $work 'source'; New-Item -ItemType Directory -Path $sourceDir | Out-Null
                $ref=if ($revision) { $revision } elseif ($version -ne 'latest') { $version } else { 'main' }
                Invoke-Checked 'git' @('-C',$sourceDir,'init','--quiet')
                Invoke-Checked 'git' @('-C',$sourceDir,'remote','add','origin','https://github.com/TinyuengKwan/sail-lsp.git')
                Invoke-Checked 'git' @('-C',$sourceDir,'fetch','--quiet','--depth','1','origin',$ref)
                Invoke-Checked 'git' @('-C',$sourceDir,'checkout','--quiet','--detach','FETCH_HEAD')
            }
        }
        $sourceDir=(Resolve-Path -LiteralPath $sourceDir).Path
        if ($revision) {
            $expected = & git -C $sourceDir rev-parse --verify "$revision^{commit}"
            if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve --rev' }
            $actual = & git -C $sourceDir rev-parse HEAD
            if ($LASTEXITCODE -ne 0 -or $actual -ne $expected) { throw 'Local checkout does not match --rev' }
            $status = & git -C $sourceDir status --porcelain --untracked-files=normal
            if ($LASTEXITCODE -ne 0 -or $status) { throw '--rev requires a clean checkout; omit it for working-tree builds' }
        }
        $oldCFlags=$env:CFLAGS; $oldCxxFlags=$env:CXXFLAGS; $oldRustFlags=$env:RUSTFLAGS; $oldTarget=$env:CARGO_BUILD_TARGET
        $invocationDirectory=(Get-Location).Path
        Push-Location $sourceDir
        try {
            $env:CFLAGS=if ($env:CFLAGS) { $env:CFLAGS+' /utf-8' } else { '/utf-8' }
            $env:CXXFLAGS=if ($env:CXXFLAGS) { $env:CXXFLAGS+' /utf-8' } else { '/utf-8' }
            if ($env:RUSTFLAGS -notmatch 'target-feature=\+crt-static') { $env:RUSTFLAGS=($env:RUSTFLAGS+' -C target-feature=+crt-static').Trim() }
            if (-not $env:CARGO_BUILD_TARGET) {
                $hostLine = & rustc -vV | Where-Object { $_ -like 'host: *' }
                if (-not $hostLine) { throw 'Cannot determine the Rust host target' }
                $env:CARGO_BUILD_TARGET = $hostLine.Substring(6).Trim()
            }
            Invoke-Checked 'cargo' @('build','--release','--locked','--features','vendored','--bin','sail-lsp')
            $targetDir=if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
            if ($env:CARGO_BUILD_TARGET) { $targetDir=Join-Path $targetDir $env:CARGO_BUILD_TARGET }
            $runner=(Resolve-Path (Join-Path $targetDir 'release/sail-lsp.exe')).Path
            $bootstrap=Join-Path $work 'source-installer.exe'
            Copy-Item -LiteralPath $runner -Destination $bootstrap
            Push-Location $invocationDirectory
            try { Invoke-Checked $bootstrap (@('install')+$installerArgs+@('--path',$sourceDir)) }
            finally { Pop-Location }
        } finally { $env:CFLAGS=$oldCFlags; $env:CXXFLAGS=$oldCxxFlags; $env:RUSTFLAGS=$oldRustFlags; $env:CARGO_BUILD_TARGET=$oldTarget; Pop-Location }
        return
    }
    $architecture=[Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    $arch=switch ($architecture) { 'X64' { 'x86_64' }; 'Arm64' { 'aarch64' }; default { throw 'No prebuilt binary for this architecture; use --from-source' } }
    $target="$arch-pc-windows-msvc"
    if (-not $archive) {
        $asset="sail-lsp-$target.zip"
        $location=if ($version -eq 'latest') { "$base/latest/download" } else { "$base/download/$version" }
        $archive=Join-Path $work $asset; Get-Asset "$location/$asset" $archive
        $sum=Join-Path $work 'checksum'; Get-Asset "$location/$asset.sha256" $sum
        $checksum=((Get-Content -LiteralPath $sum -Raw).Trim() -split '\s+')[0]
    } elseif (-not $checksum) {
        $sum="$archive.sha256"
        if (-not (Test-Path -LiteralPath $sum)) { throw 'Offline archives require a .sha256 sidecar or --sha256' }
        $checksum=((Get-Content -LiteralPath $sum -Raw).Trim() -split '\s+')[0]
    }
    if ($checksum -notmatch '^[a-fA-F0-9]{64}$') { throw 'Invalid SHA256' }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ine $checksum) { throw 'Archive SHA256 mismatch' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $unpacked=Join-Path $work 'unpacked'; New-Item -ItemType Directory -Path $unpacked | Out-Null
    $zip=[IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $archive).Path)
    try {
        $size=0L
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName -match '(^/|(^|/)\.\.(/|$)|\\)' -or (($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000) { throw 'Unsafe archive entry' }
            $destination=[IO.Path]::GetFullPath((Join-Path $unpacked $entry.FullName))
            if (-not $destination.StartsWith($unpacked+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe archive path' }
            $size+=$entry.Length; if ($size -gt 536870912) { throw 'Distribution expands beyond 512 MiB' }
        }
    } finally { $zip.Dispose() }
    [IO.Compression.ZipFile]::ExtractToDirectory((Resolve-Path -LiteralPath $archive).Path,$unpacked)
    $runner=Join-Path $unpacked 'sail-lsp.exe'
    if (-not (Test-Path -LiteralPath $runner) -or -not (Test-Path -LiteralPath (Join-Path $unpacked 'manifest.json'))) { throw 'Incomplete distribution' }
    Invoke-Checked $runner (@('install')+$installerArgs+@('--archive',$archive,'--sha256',$checksum))
} finally { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
