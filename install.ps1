$ErrorActionPreference = "Stop"

$BinaryName = "discuss"
$RepoUrl = "https://github.com/codesoda/discuss-cli"
$ApiRepoUrl = "https://api.github.com/repos/codesoda/discuss-cli"
$RawRepoUrl = "https://raw.githubusercontent.com/codesoda/discuss-cli"
$InstallDir = Join-Path $HOME ".discuss\bin"
$LinkDir = Join-Path $HOME ".local\bin"
$SkillInstallDir = Join-Path $HOME ".discuss\skills\discuss"
$InstalledBinary = Join-Path $InstallDir "$BinaryName.exe"
$ScriptDir = if ([string]::IsNullOrWhiteSpace($PSScriptRoot)) { $null } else { $PSScriptRoot }

function Fail([string] $Message) {
    Write-Error "error: $Message"
    exit 1
}

function Status([string] $Message) {
    Write-Host $Message
}

function Require-Command([string] $Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        Fail "required command '$Name' was not found"
    }
}

function Get-Target {
    $Architecture = $env:PROCESSOR_ARCHITEW6432
    if ([string]::IsNullOrWhiteSpace($Architecture)) {
        $Architecture = $env:PROCESSOR_ARCHITECTURE
    }

    if ($Architecture -ieq "AMD64") {
        return "x86_64-pc-windows-msvc"
    }

    Fail "unsupported Windows architecture '$Architecture'; the available Windows release is x86_64-pc-windows-msvc"
}

function Get-LatestReleaseTag {
    $Release = Invoke-RestMethod -Uri "$ApiRepoUrl/releases/latest" -Headers @{ "User-Agent" = "$BinaryName-installer" }
    $Tag = [string]$Release.tag_name
    if ($Tag -notmatch '^v[0-9]') {
        Fail "could not determine the latest release tag"
    }
    return $Tag
}

function Install-Binary([string] $Source) {
    New-Item -ItemType Directory -Force -Path $InstallDir, $LinkDir | Out-Null
    Copy-Item -Force $Source $InstalledBinary

    $Wrapper = Join-Path $LinkDir "$BinaryName.cmd"
    "@echo off`r`n`"$InstalledBinary`" %*`r`n" | Set-Content -Path $Wrapper -Encoding ascii

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathEntries = @($UserPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if (-not ($PathEntries | Where-Object { $_.TrimEnd('\') -ieq $LinkDir.TrimEnd('\') })) {
        $NewPath = if ([string]::IsNullOrWhiteSpace($UserPath)) { $LinkDir } else { "$UserPath;$LinkDir" }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Status "Added $LinkDir to the user PATH. Open a new terminal before running $BinaryName."
    }
}

function Install-Skill([string] $Source) {
    if (-not (Test-Path $Source -PathType Container)) {
        Write-Warning "skill source not found at $Source; skipping skill install"
        return
    }

    New-Item -ItemType Directory -Force -Path (Split-Path $SkillInstallDir) | Out-Null
    if (Test-Path $SkillInstallDir) {
        Remove-Item -Recurse -Force $SkillInstallDir
    }
    Copy-Item -Recurse $Source $SkillInstallDir

    foreach ($AgentRoot in @((Join-Path $HOME ".claude"), (Join-Path $HOME ".codex"), (Join-Path $HOME ".agents"))) {
        if (-not (Test-Path $AgentRoot -PathType Container)) {
            continue
        }

        $SkillsDir = Join-Path $AgentRoot "skills"
        $Target = Join-Path $SkillsDir $BinaryName
        New-Item -ItemType Directory -Force -Path $SkillsDir | Out-Null
        if (Test-Path $Target) {
            $Existing = Get-Item -Force $Target
            if ($Existing.LinkType) {
                Remove-Item -Force $Target
            } else {
                Write-Warning "$Target exists and is not a link; skipping"
                continue
            }
        }

        New-Item -ItemType Junction -Path $Target -Target $SkillInstallDir | Out-Null
        Status "Linked skill $Target -> $SkillInstallDir"
    }
}

function Install-FromSource {
    Require-Command "cargo"
    Status "Building $BinaryName with warnings denied..."
    Push-Location $ScriptDir
    try {
        $env:RUSTFLAGS = "-D warnings"
        & cargo build --release
        if ($LASTEXITCODE -ne 0) {
            Fail "cargo build failed"
        }
    } finally {
        Pop-Location
    }

    $Source = Join-Path $ScriptDir "target\release\$BinaryName.exe"
    if (-not (Test-Path $Source -PathType Leaf)) {
        Fail "expected built binary at $Source"
    }
    Install-Binary $Source
    Install-Skill (Join-Path $ScriptDir "skills\discuss")
}

function Install-FromDownload {
    $Target = Get-Target
    $Tag = Get-LatestReleaseTag
    $AssetName = "$BinaryName-$Tag-$Target.zip"
    $TempDir = Join-Path ([IO.Path]::GetTempPath()) ("discuss-install-" + [guid]::NewGuid().ToString("N"))
    $Archive = Join-Path $TempDir $AssetName
    $Checksums = Join-Path $TempDir "checksums-sha256.txt"

    New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
    try {
        $AssetUrl = "$RepoUrl/releases/download/$Tag/$AssetName"
        $ChecksumsUrl = "$RepoUrl/releases/download/$Tag/checksums-sha256.txt"
        Status "Downloading $AssetName..."
        Invoke-WebRequest -Uri $AssetUrl -OutFile $Archive
        Invoke-WebRequest -Uri $ChecksumsUrl -OutFile $Checksums

        $Expected = $null
        foreach ($Line in Get-Content $Checksums) {
            if ($Line -match "^(?<Hash>[0-9a-fA-F]{64})\s+\*?(?<Name>\S+)$" -and $Matches.Name -eq $AssetName) {
                $Expected = $Matches.Hash
                break
            }
        }
        if (-not $Expected) {
            Fail "checksums-sha256.txt did not contain an entry for $AssetName"
        }
        $Actual = (Get-FileHash -Path $Archive -Algorithm SHA256).Hash
        if ($Actual -ine $Expected) {
            Fail "sha256 mismatch for $AssetName; refusing to install"
        }

        $ExtractDir = Join-Path $TempDir "extracted"
        Expand-Archive -Path $Archive -DestinationPath $ExtractDir -Force
        $Source = Get-ChildItem -Path $ExtractDir -Filter "$BinaryName.exe" -Recurse -File | Select-Object -First 1
        if (-not $Source) {
            Fail "$AssetName did not contain $BinaryName.exe"
        }

        Install-Binary $Source.FullName
        $RawBase = "$RawRepoUrl/$Tag/skills/discuss"
        $Manifest = Join-Path $TempDir "manifest.txt"
        Invoke-WebRequest -Uri "$RawBase/manifest.txt" -OutFile $Manifest
        $SkillSource = Join-Path $TempDir "skill"
        New-Item -ItemType Directory -Force -Path $SkillSource | Out-Null
        foreach ($FilePath in Get-Content $Manifest) {
            $RelativePath = $FilePath.Trim()
            if ([string]::IsNullOrWhiteSpace($RelativePath) -or $RelativePath.StartsWith('#')) {
                continue
            }
            $Destination = Join-Path $SkillSource $RelativePath
            New-Item -ItemType Directory -Force -Path (Split-Path $Destination) | Out-Null
            Invoke-WebRequest -Uri "$RawBase/$RelativePath" -OutFile $Destination
        }
        Install-Skill $SkillSource
    } finally {
        if (Test-Path $TempDir) {
            Remove-Item -Recurse -Force $TempDir
        }
    }
}

if ($ScriptDir -and (Test-Path (Join-Path $ScriptDir "Cargo.toml"))) {
    Install-FromSource
} else {
    Install-FromDownload
}

if (-not (Test-Path $InstalledBinary -PathType Leaf)) {
    Fail "installed binary was not found at $InstalledBinary"
}
& $InstalledBinary --version
if ($LASTEXITCODE -ne 0) {
    Fail "installed binary failed to run"
}
Status "Installed $BinaryName to $InstalledBinary"
Status "Open a new terminal before using $BinaryName from PATH."
