# Shared helpers for the Windows release workflow.
# This file only defines functions. Entry scripts dot-source it.

function Get-WorkDirectory {
    $dir = $env:RUNNER_TEMP
    if ([string]::IsNullOrWhiteSpace($dir)) {
        $dir = [System.IO.Path]::GetTempPath()
    }
    return $dir
}

function Write-Utf8Text {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text
    )
    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $Text, $utf8)
}

function Add-Utf8Text {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text
    )
    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::AppendAllText($Path, $Text, $utf8)
}

function Get-AppVersion {
    param([Parameter(Mandatory = $true)][string]$Root)

    $packageVersion = [string]((Get-Content -Raw -LiteralPath (Join-Path $Root 'package.json') | ConvertFrom-Json).version)

    # package-lock.json uses "" as the root package key. PowerShell 7 reads that
    # with -AsHashtable. This function runs under pwsh in CI, not Windows PowerShell 5.1.
    $lockJson = Get-Content -Raw -LiteralPath (Join-Path $Root 'package-lock.json') | ConvertFrom-Json -AsHashtable
    $lockVersion = [string]$lockJson['version']
    $lockPackages = $lockJson['packages']
    if ($null -eq $lockPackages -or -not $lockPackages.ContainsKey('')) {
        throw 'package-lock.json is missing the root package version.'
    }
    $lockPackageVersion = [string]$lockPackages['']['version']

    $cargoText = Get-Content -Raw -LiteralPath (Join-Path $Root 'src-tauri/Cargo.toml')
    $cargoMatch = [regex]::Match($cargoText, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $cargoMatch.Success) {
        throw 'src-tauri/Cargo.toml has no package version.'
    }
    $cargoVersion = $cargoMatch.Groups[1].Value

    $tauriVersion = [string]((Get-Content -Raw -LiteralPath (Join-Path $Root 'src-tauri/tauri.conf.json') | ConvertFrom-Json).version)

    $names = @(
        'package.json',
        'package-lock.json',
        'package-lock.json packages',
        'src-tauri/Cargo.toml',
        'src-tauri/tauri.conf.json'
    )
    $values = @($packageVersion, $lockVersion, $lockPackageVersion, $cargoVersion, $tauriVersion)
    $distinct = @($values | Select-Object -Unique)
    if ($distinct.Count -ne 1 -or [string]::IsNullOrWhiteSpace([string]$distinct[0])) {
        $detail = @()
        for ($i = 0; $i -lt $names.Count; $i++) {
            $detail += "$($names[$i])=$($values[$i])"
        }
        throw ("Version files disagree: " + ($detail -join ', '))
    }
    return [string]$distinct[0]
}

function Assert-TagMatchesVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Tag,
        [Parameter(Mandatory = $true)][string]$Version
    )
    $expected = "v$Version"
    if ($Tag -ne $expected) {
        throw "Tag '$Tag' has to be $expected so it matches package.json, package-lock.json, src-tauri/Cargo.toml, and src-tauri/tauri.conf.json."
    }
}

function Get-SigningMethod {
    if ($env:SIGNING_ENABLED -ne 'true') {
        return 'off'
    }
    $method = $env:SIGNING_METHOD
    if ([string]::IsNullOrWhiteSpace($method)) {
        return 'pfx'
    }
    # GitHub's vars comparison is case-sensitive, and so is this check.
    $ordinal = [System.StringComparison]::Ordinal
    if ([string]::Equals($method, 'pfx', $ordinal) -or [string]::Equals($method, 'azure', $ordinal)) {
        return $method
    }
    throw 'SIGNING_METHOD must be empty, pfx, or azure. Refusing to sign.'
}

function Assert-AzureDlibPin {
    param([string]$Url, [string]$Hash)
    if ([string]::IsNullOrWhiteSpace($Url) -or $Url -notmatch '^https://') {
        throw 'AZURE_DLIB_URL must be an https URL. Refusing to download a signing tool.'
    }
    if ([string]::IsNullOrWhiteSpace($Hash) -or $Hash.ToLowerInvariant() -notmatch '^[0-9a-f]{64}$') {
        throw 'AZURE_DLIB_SHA256 must be 64 hex characters. Refusing to download a signing tool.'
    }
}

function Assert-SigningMaterial {
    param([Parameter(Mandatory = $true)][string]$Method)
    $ordinal = [System.StringComparison]::Ordinal
    if ([string]::Equals($Method, 'pfx', $ordinal)) {
        if ([string]::IsNullOrWhiteSpace($env:WINDOWS_CERTIFICATE)) {
            throw 'SIGNING_METHOD is pfx but WINDOWS_CERTIFICATE is empty. Add that secret on the release environment. Refusing to produce a signed build.'
        }
        if ([string]::IsNullOrWhiteSpace($env:WINDOWS_CERTIFICATE_PASSWORD)) {
            throw 'SIGNING_METHOD is pfx but WINDOWS_CERTIFICATE_PASSWORD is empty. Refusing to produce a signed build.'
        }
        return
    }
    if ([string]::Equals($Method, 'azure', $ordinal)) {
        $names = @(
            'AZURE_TENANT_ID',
            'AZURE_CLIENT_ID',
            'AZURE_CLIENT_SECRET',
            'AZURE_CODE_SIGNING_ACCOUNT_NAME',
            'AZURE_CERT_PROFILE_NAME',
            'AZURE_ENDPOINT',
            'AZURE_DLIB_URL',
            'AZURE_DLIB_SHA256'
        )
        $missing = New-Object System.Collections.Generic.List[string]
        foreach ($name in $names) {
            $value = [Environment]::GetEnvironmentVariable($name)
            if ([string]::IsNullOrWhiteSpace($value)) {
                $missing.Add($name)
            }
        }
        if ($missing.Count -gt 0) {
            throw ("SIGNING_METHOD is azure but these values are empty: " + ($missing -join ', ') + '. Refusing to produce a signed build.')
        }
        Assert-AzureDlibPin -Url $env:AZURE_DLIB_URL -Hash $env:AZURE_DLIB_SHA256
        return
    }
    throw 'Signing is off. Refusing to read certificate material.'
}

function Install-AzureDlib {
    $url = $env:AZURE_DLIB_URL
    $hash = $env:AZURE_DLIB_SHA256
    Assert-AzureDlibPin -Url $url -Hash $hash

    $work = Join-Path (Get-WorkDirectory) 'cfw-azure-dlib'
    if (Test-Path -LiteralPath $work) {
        Remove-Item -LiteralPath $work -Recurse -Force
    }
    New-Item -ItemType Directory -Path $work | Out-Null
    $download = Join-Path $work 'download.bin'
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $url -OutFile $download -UseBasicParsing
    } catch {
        throw 'The Azure signing tool download failed. Check the AZURE_DLIB_URL repository variable. The URL is not repeated here.'
    }
    $length = (Get-Item -LiteralPath $download).Length
    if ($length -gt 50MB) {
        Remove-Item -LiteralPath $download -Force
        throw 'The Azure signing tool download is larger than 50 MB. Refusing it.'
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $download).Hash.ToLowerInvariant()
    $expected = $hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Remove-Item -LiteralPath $download -Force
        throw "The Azure signing tool hash was $actual. The pinned hash does not match. Refusing to sign."
    }

    $extract = Join-Path $work 'files'
    New-Item -ItemType Directory -Path $extract | Out-Null
    $probe = [System.IO.File]::OpenRead($download)
    try {
        $b0 = $probe.ReadByte()
        $b1 = $probe.ReadByte()
    } finally {
        $probe.Dispose()
    }
    if ($b0 -eq 0x50 -and $b1 -eq 0x4B) {
        Expand-Archive -LiteralPath $download -DestinationPath $extract -Force
    } else {
        Copy-Item -LiteralPath $download -Destination (Join-Path $extract 'Azure.CodeSigning.Dlib.dll')
    }

    $dlls = New-Object System.Collections.Generic.List[System.IO.FileInfo]
    $found = Get-ChildItem -LiteralPath $extract -Filter 'Azure.CodeSigning.Dlib.dll' -Recurse -File -ErrorAction SilentlyContinue
    foreach ($dll in @($found)) {
        if ($null -ne $dll) {
            $dlls.Add($dll)
        }
    }
    $x64 = @($dlls | Where-Object { $_.FullName -match '[\\/]x64[\\/]' })
    $chosen = $null
    if ($x64.Count -eq 1 -and $null -ne $x64[0]) {
        $chosen = $x64[0]
    } elseif ($dlls.Count -eq 1) {
        $chosen = $dlls[0]
    } else {
        throw "Expected one Azure.CodeSigning.Dlib.dll, or exactly one under an x64 folder. Found $($dlls.Count)."
    }
    $env:AZURE_DLIB_PATH = $chosen.FullName
    [Environment]::SetEnvironmentVariable('AZURE_DLIB_PATH', $chosen.FullName)
    Write-Output 'Azure signing tool hash matched.'
}

function Get-InstallerFiles {
    param([Parameter(Mandatory = $true)][string]$BundleRoot)
    $list = New-Object System.Collections.Generic.List[System.IO.FileInfo]
    $searches = @(
        @{ Dir = (Join-Path $BundleRoot 'msi'); Filter = '*.msi' },
        @{ Dir = (Join-Path $BundleRoot 'nsis'); Filter = '*.exe' }
    )
    foreach ($search in $searches) {
        if (-not (Test-Path -LiteralPath $search.Dir)) {
            continue
        }
        $found = Get-ChildItem -LiteralPath $search.Dir -Filter $search.Filter -File -ErrorAction SilentlyContinue
        foreach ($file in @($found)) {
            if ($null -ne $file) {
                $list.Add($file)
            }
        }
    }
    foreach ($file in $list) {
        Write-Output $file
    }
}

function Write-Sha256Sums {
    param([Parameter(Mandatory = $true)][string]$Directory)
    $files = New-Object System.Collections.Generic.List[System.IO.FileInfo]
    $found = Get-ChildItem -LiteralPath $Directory -File -ErrorAction SilentlyContinue
    foreach ($file in @($found)) {
        if ($null -eq $file) { continue }
        if ($file.Name -eq 'SHA256SUMS' -or $file.Name -eq 'RELEASE_NOTES.md') { continue }
        $files.Add($file)
    }
    $sorted = @($files.ToArray() | Sort-Object Name)
    if ($sorted.Count -lt 1 -or $null -eq $sorted[0]) {
        throw "No files to hash in $Directory"
    }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $lines = New-Object System.Collections.Generic.List[string]
    try {
        foreach ($file in $sorted) {
            if ($null -eq $file) { continue }
            $stream = [System.IO.File]::OpenRead($file.FullName)
            try {
                $hash = $sha.ComputeHash($stream)
            } finally {
                $stream.Dispose()
            }
            $hex = ([System.BitConverter]::ToString($hash)).Replace('-', '').ToLowerInvariant()
            $lines.Add("$hex  $($file.Name)")
        }
    } finally {
        $sha.Dispose()
    }
    $text = ($lines.ToArray() -join "`n") + "`n"
    Write-Utf8Text -Path (Join-Path $Directory 'SHA256SUMS') -Text $text
}

function Copy-ReleaseFiles {
    param(
        [Parameter(Mandatory = $true)][string]$BundleRoot,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][bool]$Signed
    )
    if (Test-Path -LiteralPath $Destination) {
        Remove-Item -LiteralPath $Destination -Recurse -Force
    }
    New-Item -ItemType Directory -Path $Destination | Out-Null

    $files = @(Get-InstallerFiles -BundleRoot $BundleRoot)
    $msi = @($files | Where-Object { $null -ne $_ -and $_.Extension -eq '.msi' })
    $exe = @($files | Where-Object { $null -ne $_ -and $_.Extension -eq '.exe' })
    if ($msi.Count -lt 1 -or $null -eq $msi[0] -or $exe.Count -lt 1 -or $null -eq $exe[0]) {
        $foundNames = @($files | Where-Object { $null -ne $_ } | ForEach-Object { $_.Name })
        throw ("The bundle needs at least one .msi and one .exe. Found: " + ($foundNames -join ', '))
    }
    foreach ($file in $files) {
        if ($null -eq $file) { continue }
        Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $Destination $file.Name)
    }
    $label = 'unsigned'
    if ($Signed) { $label = 'signed' }
    Write-Utf8Text -Path (Join-Path $Destination 'SIGNING.txt') -Text ($label + "`n")
    Write-Sha256Sums -Directory $Destination
}

function New-TauriSigningConfigJson {
    param(
        [Parameter(Mandatory = $true)][string]$ScriptPath,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    # Tauri replaces the %1 argument with the file it wants signed.
    # https://v2.tauri.app/reference/config/#signcommand
    $config = [ordered]@{
        bundle = [ordered]@{
            windows = [ordered]@{
                signCommand = [ordered]@{
                    cmd  = 'powershell.exe'
                    args = @(
                        '-NoProfile',
                        '-ExecutionPolicy',
                        'Bypass',
                        '-File',
                        $ScriptPath,
                        '%1'
                    )
                }
            }
        }
    }
    $json = $config | ConvertTo-Json -Depth 8
    Write-Utf8Text -Path $Destination -Text $json
    return $json
}

function Find-SignTool {
    $override = $env:TAURI_WINDOWS_SIGNTOOL_PATH
    if (-not [string]::IsNullOrWhiteSpace($override)) {
        if (Test-Path -LiteralPath $override) {
            return (Resolve-Path -LiteralPath $override).Path
        }
        throw 'TAURI_WINDOWS_SIGNTOOL_PATH is set, and that file was not found.'
    }
    $roots = New-Object System.Collections.Generic.List[string]
    $pf86 = ${env:ProgramFiles(x86)}
    if (-not [string]::IsNullOrWhiteSpace($pf86)) {
        $roots.Add((Join-Path $pf86 'Windows Kits\10'))
    }
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $roots.Add((Join-Path $env:ProgramFiles 'Windows Kits\10'))
    }
    $found = New-Object System.Collections.Generic.List[System.IO.FileInfo]
    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        $hits = Get-ChildItem -LiteralPath $root -Filter signtool.exe -Recurse -File -ErrorAction SilentlyContinue
        foreach ($hit in @($hits)) {
            if ($null -ne $hit) { $found.Add($hit) }
        }
    }
    $x64 = @($found | Where-Object { $_.FullName -match '[\\/]x64[\\/]signtool\.exe$' })
    $pool = @($found.ToArray())
    if ($x64.Count -ge 1 -and $null -ne $x64[0]) {
        $pool = $x64
    }
    $best = $pool | Sort-Object FullName -Descending | Select-Object -First 1
    if ($null -eq $best) { return $null }
    return $best.FullName
}

function Assert-AuthenticodeResults {
    param(
        [Parameter(Mandatory = $true)][bool]$Signed,
        [Parameter(Mandatory = $true)][bool]$FoundSignTool,
        [Parameter(Mandatory = $true)][bool]$RunningOnActions,
        [object[]]$Results
    )
    if (-not $FoundSignTool) {
        if ($Signed) {
            throw 'signtool.exe was not found. Refusing to call this build signed.'
        }
        if ($RunningOnActions) {
            throw 'signtool.exe was not found, so this Actions build cannot show that it is unsigned.'
        }
        return 'skipped'
    }
    $rows = @($Results | Where-Object { $null -ne $_ })
    if ($rows.Count -lt 1 -or $null -eq $rows[0]) {
        throw 'There was no .exe or .msi to check with signtool verify /pa.'
    }
    foreach ($result in $rows) {
        $code = [int]$result.ExitCode
        if ($Signed -and $code -ne 0) {
            throw "signtool verify /pa failed for $($result.Name). Refusing to call this build signed."
        }
        if (-not $Signed -and $code -eq 0) {
            throw "signtool verify /pa succeeded for $($result.Name). Refusing to label this build unsigned."
        }
    }
    return 'checked'
}

function New-ReleaseSummary {
    param(
        [Parameter(Mandatory = $true)][bool]$Signed,
        [string[]]$FileNames,
        [string]$VerifyNote,
        [string]$SignStepOutcome
    )
    $names = @($FileNames | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $list = '(none)'
    if ($names.Count -gt 0 -and $null -ne $names[0]) {
        $list = ($names | ForEach-Object { "- $_" }) -join "`n"
    }
    $parts = New-Object System.Collections.Generic.List[string]
    if ($Signed) {
        $heading = 'This build is SIGNED.'
        $parts.Add('SIGNING_ENABLED is true.')
        $parts.Add('Tauri signCommand ran scripts/sign-windows.ps1.')
        $parts.Add('Certificate material was read from the release environment.')
        if (-not [string]::IsNullOrWhiteSpace($VerifyNote)) { $parts.Add($VerifyNote) }
        $parts.Add('A tag push opens a draft GitHub release. You publish that draft from the releases page.')
    } else {
        $heading = 'This build is UNSIGNED.'
        $parts.Add('SIGNING_ENABLED is not true, so signing stayed off.')
        if ($SignStepOutcome -eq 'skipped') {
            $parts.Add('The Sign Windows binaries step was skipped. Tauri ran without a signCommand.')
        } else {
            $parts.Add('Tauri ran without a signCommand.')
        }
        $parts.Add('This installer has no Authenticode signature. Windows SmartScreen will warn.')
        $parts.Add('Compare the files with SHA256SUMS before you run them.')
        if (-not [string]::IsNullOrWhiteSpace($VerifyNote)) { $parts.Add($VerifyNote) }
        $parts.Add('This job did not publish a GitHub release.')
    }
    $body = $parts.ToArray() -join "`n`n"
    return @"
## $heading

$body

Files:

$list
"@
}

function New-ReleaseNotes {
    param(
        [Parameter(Mandatory = $true)][string]$Heading,
        [Parameter(Mandatory = $true)][bool]$Signed,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$SumsText
    )
    if ($Signed) {
        $banner = '**This build is SIGNED.** Check `signtool verify /pa` before you publish. This page is a draft.'
    } else {
        $banner = '**This build is UNSIGNED.** Signing is off. Windows SmartScreen will warn. This page is a draft.'
    }
    $template = @'
# __HEADING__

__BANNER__

Publishing this draft is a separate step. Open the release in GitHub and click Publish after the checks below.

## Check SHA256SUMS

`SHA256SUMS` is attached next to the installers. On Windows, hash a file and compare it to the matching line. PowerShell prints uppercase letters. The file uses lowercase. Compare the letters without caring about case.

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath '.\the-setup.exe'
```

In a folder that contains the files and GNU sha256sum:

```bash
sha256sum -c SHA256SUMS
```

## Check Authenticode

`signtool.exe` ships with the Windows SDK. `/pa` uses the default Authenticode policy, which is the trust check Windows itself uses.

```bat
signtool verify /pa "the-setup.exe"
signtool verify /pa "the-setup.msi"
```

Use the real file names from `SHA256SUMS`. An unsigned file fails this check. A signed file reports that it was successfully verified. Check the setup `.exe` and the `.msi`.

`SIGNING.txt` says either `unsigned` or `signed`.

## Hashes

```text
__SUMS__```
'@
    return $template.Replace('__HEADING__', $Heading).Replace('__BANNER__', $banner).Replace('__SUMS__', $SumsText)
}
