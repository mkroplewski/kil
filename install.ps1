[CmdletBinding()]
param(
    [string]$InstallRoot = "$env:LOCALAPPDATA\kil"
)

$ErrorActionPreference = "Stop"
$repository = "mkroplewski/kil"

if (-not [Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -notin @("AMD64", "x86_64")) {
    throw "kil currently provides a Windows binary for x86-64 only"
}

$asset = "kil-x86_64-pc-windows-msvc.zip"
$releaseBase = "https://github.com/$repository/releases/latest/download"
$temporaryDir = Join-Path ([IO.Path]::GetTempPath()) ("kil-install-" + [guid]::NewGuid())
$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
if ($InstallRoot -eq [IO.Path]::GetPathRoot($InstallRoot) -or
    $InstallRoot.TrimEnd('\') -eq $env:USERPROFILE.TrimEnd('\')) {
    throw "InstallRoot must be a dedicated subdirectory"
}

try {
    New-Item -ItemType Directory -Path $temporaryDir | Out-Null
    $archive = Join-Path $temporaryDir $asset
    $checksums = Join-Path $temporaryDir "SHA256SUMS"
    $payload = Join-Path $temporaryDir "payload"

    Invoke-WebRequest -Uri "$releaseBase/$asset" -OutFile $archive -UseBasicParsing
    Invoke-WebRequest -Uri "$releaseBase/SHA256SUMS" -OutFile $checksums -UseBasicParsing

    $expectedLine = Get-Content $checksums | Where-Object { $_ -match "\s+$([regex]::Escape($asset))$" } | Select-Object -First 1
    if (-not $expectedLine) {
        throw "release checksum for $asset was not found"
    }
    $expected = ($expectedLine -split "\s+")[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "checksum mismatch for $asset"
    }

    Expand-Archive -Path $archive -DestinationPath $payload -Force
    if (-not (Test-Path -LiteralPath (Join-Path $payload "kil.exe")) -or
        -not (Test-Path -LiteralPath (Join-Path $payload "krt\py_router\route.py"))) {
        throw "release archive is missing kil or KiCadRoutingTools"
    }

    $binDir = Join-Path $InstallRoot "bin"
    $krtDir = Join-Path $InstallRoot "lib\krt"
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
    New-Item -ItemType Directory -Path (Split-Path $krtDir) -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $payload "kil.exe") -Destination (Join-Path $binDir "kil.exe") -Force
    if (Test-Path -LiteralPath $krtDir) {
        Remove-Item -LiteralPath $krtDir -Recurse -Force
    }
    Copy-Item -LiteralPath (Join-Path $payload "krt") -Destination $krtDir -Recurse

    $pythonCandidates = @(
        @{ Program = "python"; Args = @() },
        @{ Program = "py"; Args = @("-3") },
        @{ Program = "python3"; Args = @() }
    )
    $python = $null
    foreach ($candidate in $pythonCandidates) {
        try {
            $candidateArgs = $candidate.Args
            & $candidate.Program @candidateArgs -c "import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)" 2>$null
            if ($LASTEXITCODE -eq 0) {
                $python = $candidate
                break
            }
        }
        catch {}
    }
    if (-not $python) {
        throw "Python 3.9 or newer is required for the bundled autorouter"
    }

    $venvDir = Join-Path $InstallRoot "python"
    if (-not (Test-Path -LiteralPath (Join-Path $venvDir "Scripts\python.exe"))) {
        $pythonArgs = $python.Args
        & $python.Program @pythonArgs -m venv $venvDir
        if ($LASTEXITCODE -ne 0) {
            throw "could not create the private Python environment"
        }
    }
    $venvPython = Join-Path $venvDir "Scripts\python.exe"
    & $venvPython -m pip install --disable-pip-version-check --quiet --upgrade -r (Join-Path $krtDir "requirements.txt")
    if ($LASTEXITCODE -ne 0) {
        throw "could not install KiCadRoutingTools Python dependencies"
    }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $pathEntries = @($userPath -split ";" | Where-Object { $_ })
    if ($pathEntries -notcontains $binDir) {
        [Environment]::SetEnvironmentVariable("Path", ((@($pathEntries) + $binDir) -join ";"), "User")
    }

    Write-Host "Installed kil and KiCadRoutingTools to $InstallRoot"
    Write-Host "Open a new terminal, then run: kil --version"
}
finally {
    if (Test-Path -LiteralPath $temporaryDir) {
        Remove-Item -LiteralPath $temporaryDir -Recurse -Force
    }
}
