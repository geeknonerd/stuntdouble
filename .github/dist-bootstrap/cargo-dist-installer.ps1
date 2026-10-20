#!/usr/bin/env pwsh
# Download and verify the pinned cargo-dist installer.
$ErrorActionPreference = "Stop"

$DistVersion = "0.33.0"
$InstallerSha256 = "2cf34763a6f890c05793702993f6cf9732ff497651c87a9965a8548fd68785fb"

function Get-RepoRoot {
    if ($PSScriptRoot) {
        return Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    }
    # cargo-dist invokes this script through `irm ... | iex`, where
    # $PSScriptRoot is empty and the workflow's working directory is the root.
    return (Get-Location).Path
}

function Get-CargoDistVersion {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "cannot read $Path"
    }

    $declarations = @()
    foreach ($rawLine in Get-Content -LiteralPath $Path) {
        $line = $rawLine.Trim()
        if ($line -cmatch '^cargo-dist-version\s*=') {
            $declarations += $line
        }
    }
    if ($declarations.Count -eq 0) {
        throw "$Path is missing cargo-dist-version"
    }
    if ($declarations.Count -ne 1) {
        throw "$Path must contain exactly one cargo-dist-version"
    }
    if ($declarations[0] -cnotmatch '^cargo-dist-version\s*=\s*"([^"]+)"\s*(?:#.*)?$') {
        throw "malformed cargo-dist-version in $Path"
    }
    $version = $Matches[1]
    if ($version -notmatch '^[0-9A-Za-z.+-]+$') {
        throw "invalid cargo-dist-version: $version"
    }
    return $version
}

function Assert-Version {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Expected
    )
    $actual = Get-CargoDistVersion -Path $Path
    if ($actual -cne $Expected) {
        throw "$Path declares cargo-dist $actual, but the bootstrap wrapper pins $Expected"
    }
}

function Test-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Value)
    if ($Value -cmatch '^[0-9a-f]{64}$') {
        return $true
    }
    return $false
}

function Assert-Sha256 {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Expected
    )
    if (-not (Test-Sha256 -Value $Expected)) {
        throw "invalid expected SHA-256: $Expected"
    }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -cne $Expected) {
        throw "SHA-256 mismatch for ${Path}: expected $Expected, got $actual"
    }
}

function Invoke-SelfTest {
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("cargo-dist-bootstrap-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $config = Join-Path $tmp "dist-workspace.toml"
        $duplicateConfig = Join-Path $tmp "duplicate-dist-workspace.toml"
        $mismatchConfig = Join-Path $tmp "mismatch-dist-workspace.toml"
        $fixture = Join-Path $tmp "installer-fixture"
        $knownSha256 = "b4bde33f55012026d69eb6c73ae6b1b06951bf9558f1b2e5568176adda4090ce"
        $ascii = New-Object System.Text.ASCIIEncoding

        [System.IO.File]::WriteAllText($config, "cargo-dist-version = `"$DistVersion`"`r`n", $ascii)
        if ((Get-CargoDistVersion -Path $config) -cne $DistVersion) {
            throw "self-test: cargo-dist-version parsing failed"
        }
        Assert-Version -Path $config -Expected $DistVersion

        [System.IO.File]::WriteAllText(
            $duplicateConfig,
            "cargo-dist-version = `"0.33.0`"`ncargo-dist-version = `"0.34.0`"`n",
            $ascii
        )
        $duplicateRejected = $false
        try {
            Get-CargoDistVersion -Path $duplicateConfig | Out-Null
        } catch {
            $duplicateRejected = $true
        }
        if (-not $duplicateRejected) {
            throw "self-test: duplicate cargo-dist-version was accepted"
        }

        [System.IO.File]::WriteAllText($mismatchConfig, "cargo-dist-version = `"9.8.7`"`n", $ascii)
        $versionMismatchRejected = $false
        try {
            Assert-Version -Path $mismatchConfig -Expected $DistVersion
        } catch {
            $versionMismatchRejected = $true
        }
        if (-not $versionMismatchRejected) {
            throw "self-test: bootstrap version mismatch was accepted"
        }

        if (-not (Test-Sha256 -Value $InstallerSha256)) {
            throw "self-test: embedded installer SHA-256 is invalid"
        }
        if (Test-Sha256 -Value $InstallerSha256.ToUpperInvariant()) {
            throw "self-test: uppercase SHA-256 was accepted"
        }

        [System.IO.File]::WriteAllText($fixture, "cargo-dist-bootstrap`n", $ascii)
        Assert-Sha256 -Path $fixture -Expected $knownSha256

        $mismatchRejected = $false
        try {
            Assert-Sha256 -Path $fixture -Expected ("0" * 64)
        } catch {
            $mismatchRejected = $true
        }
        if (-not $mismatchRejected) {
            throw "self-test: mismatched SHA-256 was accepted"
        }

        Write-Output "self-test passed"
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$checkOnly = $args -contains "-Check"
$selfTest = $args -contains "-SelfTest"
$unknownArgs = @($args | Where-Object { $_ -ne "-Check" -and $_ -ne "-SelfTest" })
if ($unknownArgs.Count -gt 0) {
    throw "usage: cargo-dist-installer.ps1 [-Check | -SelfTest]"
}
if ($checkOnly -and $selfTest) {
    throw "usage: cargo-dist-installer.ps1 [-Check | -SelfTest]"
}
if ($selfTest) {
    Invoke-SelfTest
    exit 0
}

$root = Get-RepoRoot
$configPath = Join-Path $root "dist-workspace.toml"
Assert-Version -Path $configPath -Expected $DistVersion
if (-not (Test-Sha256 -Value $InstallerSha256)) {
    throw "invalid embedded PowerShell installer SHA-256 for cargo-dist $DistVersion"
}
$installerPath = Join-Path ([System.IO.Path]::GetTempPath()) (
    "cargo-dist-installer-" + [guid]::NewGuid().ToString("N") + ".ps1"
)
$installerUrl = "https://github.com/axodotdev/cargo-dist/releases/download/v${DistVersion}/cargo-dist-installer.ps1"

try {
    Write-Host "Downloading cargo-dist $DistVersion installer..."
    Invoke-WebRequest -Uri $installerUrl -OutFile $installerPath -UseBasicParsing -TimeoutSec 120
    Assert-Sha256 -Path $installerPath -Expected $InstallerSha256

    if ($checkOnly) {
        Write-Output "verified cargo-dist $DistVersion PowerShell installer"
        exit 0
    }

    $installer = Get-Content -LiteralPath $installerPath -Raw
    Invoke-Expression $installer
} finally {
    Remove-Item -LiteralPath $installerPath -Force -ErrorAction SilentlyContinue
}
