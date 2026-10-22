# Shared helpers for the Windows release workflow.
# This file only defines functions. Entry scripts dot-source it.

function Get-WorkDirectory {
    $dir = $env:RUNNER_TEMP
    if ([string]::IsNullOrWhiteSpace($dir)) {
        $dir = [System.IO.Path]::GetTempPath()
    }
    return $dir
}

function Get-CodeSignThumbFile {
    return (Join-Path (Get-WorkDirectory) 'cfw-codesign.thumbs')
}

function Get-CodeSignKeyFile {
    return (Join-Path (Get-WorkDirectory) 'cfw-codesign.keys')
}

function Get-CertificateKeyContainerName {
    param($Cert)
    if ($null -eq $Cert) { return $null }
    $getters = @(
        { param($Item) [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($Item) },
        { param($Item) [System.Security.Cryptography.X509Certificates.ECDsaCertificateExtensions]::GetECDsaPrivateKey($Item) }
    )
    foreach ($getter in $getters) {
        $key = $null
        try {
            $key = & $getter $Cert
        } catch {
            continue
        }
        if ($null -eq $key) { continue }
        try {
            $property = $key.PSObject.Properties['Key']
            if ($null -ne $property -and $null -ne $property.Value) {
                $unique = ([string]$property.Value.UniqueName).Trim()
                if (-not [string]::IsNullOrWhiteSpace($unique)) { return $unique }
            }
        } finally {
            $key.Dispose()
        }
    }
    return $null
}

function Save-ImportedKeyContainer {
    param([Parameter(Mandatory = $true)][string]$Name)
    $trimmed = $Name.Trim()
    if ($trimmed -notmatch '^[A-Za-z0-9._\{\}-]{1,200}$') {
        throw 'The imported key container name was unexpected. Refusing to keep it.'
    }
    $path = Get-CodeSignKeyFile
    $existing = ''
    if (Test-Path -LiteralPath $path) {
        $existing = [System.IO.File]::ReadAllText($path)
    }
    if ($existing -notmatch ("(?m)^" + [regex]::Escape($trimmed) + "$")) {
        Write-Utf8Text -Path $path -Text ($existing + $trimmed + "`n")
    }
}

function Remove-ImportedKeyContainers {
    $path = Get-CodeSignKeyFile
    if (-not (Test-Path -LiteralPath $path)) { return }
    if ($IsWindows) {
        $lines = @(Get-Content -LiteralPath $path -ErrorAction SilentlyContinue)
        $provider = [System.Security.Cryptography.CngProvider]::MicrosoftSoftwareKeyStorageProvider
        foreach ($line in $lines) {
            $name = ([string]$line).Trim()
            if ($name -notmatch '^[A-Za-z0-9._\{\}-]{1,200}$') { continue }
            if ([System.Security.Cryptography.CngKey]::Exists($name, $provider)) {
                $key = [System.Security.Cryptography.CngKey]::Open($name, $provider)
                try {
                    $key.Delete()
                    Write-Host "Removed imported code-signing key container"
                } finally {
                    $key.Dispose()
                }
            }
        }
    }
    Remove-Item -LiteralPath $path -Force
}

function Save-ImportedCertificateThumbprints {
    param([Parameter(Mandatory = $true)]$Imported)
    $thumbs = New-Object System.Collections.Generic.List[string]
    foreach ($cert in @($Imported)) {
        if ($null -eq $cert) { continue }
        $thumb = ([string]$cert.Thumbprint).Trim().ToUpperInvariant()
        if ($thumb -match '^[0-9A-F]{40,64}$') {
            $thumbs.Add($thumb)
        }
    }
    if ($thumbs.Count -lt 1) { return }
    Write-Utf8Text -Path (Get-CodeSignThumbFile) -Text (($thumbs.ToArray() -join "`n") + "`n")
}

function Remove-ImportedCertificateFromStore {
    $path = Get-CodeSignThumbFile
    if (-not (Test-Path -LiteralPath $path)) { return }
    $lines = @(Get-Content -LiteralPath $path -ErrorAction SilentlyContinue)
    $store = $null
    $opened = $false
    try {
        $store = New-Object System.Security.Cryptography.X509Certificates.X509Store(
            [System.Security.Cryptography.X509Certificates.StoreName]::My,
            [System.Security.Cryptography.X509Certificates.StoreLocation]::CurrentUser)
        $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
        $opened = $true
        foreach ($line in $lines) {
            $thumb = ([string]$line).Trim().ToUpperInvariant()
            if ($thumb -notmatch '^[0-9A-F]{40,64}$') { continue }
            $found = $store.Certificates.Find(
                [System.Security.Cryptography.X509Certificates.X509FindType]::FindByThumbprint,
                $thumb,
                $false)
            foreach ($cert in @($found)) {
                if ($null -eq $cert) { continue }
                $store.Remove($cert)
                Write-Host "Removed imported code-signing certificate $thumb"
            }
        }
    } catch {
        if ($IsWindows) {
            throw
        }
    } finally {
        if ($null -ne $store) {
            $store.Close()
        }
    }
    if ($opened -or -not $IsWindows) {
        Remove-Item -LiteralPath $path -Force
    }
}

function Remove-ImportedCodeSigningCertificate {
    try {
        Remove-ImportedCertificateFromStore
    } finally {
        Remove-ImportedKeyContainers
    }
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

function Test-ReleaseTagShape {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Tag)
    return [regex]::IsMatch($Tag, '^v\d+\.\d+\.\d+$')
}

function Assert-ReleaseTagShape {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Tag)
    if (-not (Test-ReleaseTagShape -Tag $Tag)) {
        throw "Tag '$Tag' has to look like v1.2.3. A pre-release suffix is not a release tag."
    }
}

function Assert-TagMatchesVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Tag,
        [Parameter(Mandatory = $true)][string]$Version
    )
    Assert-ReleaseTagShape -Tag $Tag
    if (-not [regex]::IsMatch($Version, '^\d+\.\d+\.\d+$')) {
        throw "Version '$Version' has to look like 1.2.3 so the tag can be v plus that number."
    }
    $expected = "v$Version"
    if (-not [string]::Equals($Tag, $expected, [StringComparison]::Ordinal)) {
        throw "Tag '$Tag' has to be $expected so it matches package.json, package-lock.json, src-tauri/Cargo.toml, and src-tauri/tauri.conf.json."
    }
}

function Get-SevenZipMsiUrl {
    # 7-Zip 26.05 x64 MSI. https://www.7-zip.org/download.html links the
    # official ip7z/7zip GitHub release. The pin is a constant so a
    # repository variable cannot retarget it.
    return 'https://github.com/ip7z/7zip/releases/download/26.05/7z2605-x64.msi'
}

function Get-SevenZipMsiSha256 {
    # SHA-256 of that asset (2008576 bytes).
    return '3fe4ab99fa15629c652944309a2b2bc6e41fc933fc6b63f5df1ebcf3aea68263'
}

function Get-SevenZipMsiMaxBytes {
    return 5242880
}

function Assert-SevenZipDownloadPin {
    $url = Get-SevenZipMsiUrl
    $uri = $null
    if (-not [Uri]::TryCreate($url, [UriKind]::Absolute, [ref]$uri)) {
        throw 'The 7-Zip download pin is not an absolute URL.'
    }
    if (-not [string]::Equals($uri.Scheme, 'https', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The 7-Zip download pin must use https.'
    }
    if (-not [string]::Equals($uri.Host, 'github.com', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The 7-Zip download pin must be hosted on github.com.'
    }
    if (-not [string]::Equals($uri.AbsolutePath, '/ip7z/7zip/releases/download/26.05/7z2605-x64.msi', [StringComparison]::Ordinal)) {
        throw 'The 7-Zip download pin must be the 26.05 x64 MSI from ip7z/7zip.'
    }
    $hash = Get-SevenZipMsiSha256
    if ($hash -notmatch '^[0-9a-f]{64}$') {
        throw 'The 7-Zip hash pin must be 64 lowercase hex characters.'
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
    Write-ReleaseLabelAndSums -Directory $Destination -Signed $Signed
}

function Stage-Installers {
    param(
        [Parameter(Mandatory = $true)][string]$BundleRoot,
        [Parameter(Mandatory = $true)][string]$Destination
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
}

function Write-ReleaseLabelAndSums {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)][bool]$Signed
    )
    $label = 'unsigned'
    if ($Signed) { $label = 'signed' }
    Write-Utf8Text -Path (Join-Path $Directory 'SIGNING.txt') -Text ($label + "`n")
    Write-Sha256Sums -Directory $Directory
}

function Get-RequiredExecutableNames {
    # The cargo bins. The installer display name is "CFW Card Studio"; the file
    # next to cfw-flash.exe is cfw-zero-touch.exe.
    return @('cfw-zero-touch.exe', 'cfw-flash.exe')
}

function Assert-ProductExecutableNames {
    param([Parameter(Mandatory = $true)][string[]]$Names)
    $present = @($Names | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    foreach ($required in (Get-RequiredExecutableNames)) {
        if ($present -notcontains $required) {
            throw "The release executables must include $required."
        }
    }
}

function Get-FileSha256Hex {
    param([Parameter(Mandatory = $true)][string]$Path)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($Path)
        try {
            $hash = $sha.ComputeHash($stream)
        } finally {
            $stream.Dispose()
        }
        return ([System.BitConverter]::ToString($hash)).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Format-HashManifest {
    param([Parameter(Mandatory = $true)][System.IO.FileInfo[]]$Files)
    $sorted = @($Files | Where-Object { $null -ne $_ } | Sort-Object Name)
    if ($sorted.Count -lt 1 -or $null -eq $sorted[0]) {
        throw 'There are no files to hash.'
    }
    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($file in $sorted) {
        if ($null -eq $file) { continue }
        $hex = Get-FileSha256Hex -Path $file.FullName
        $lines.Add("$hex  $($file.Name)")
    }
    return ($lines.ToArray() -join "`n") + "`n"
}

function ConvertTo-HashManifestBase64 {
    param([Parameter(Mandatory = $true)][string]$Manifest)
    return [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($Manifest))
}

function ConvertFrom-HashManifestBase64 {
    param([Parameter(Mandatory = $true)][string]$Encoded)
    $compact = $Encoded -replace '\s', ''
    try {
        $bytes = [Convert]::FromBase64String($compact)
    } catch {
        throw 'The hash list is not valid base64. Refusing to sign.'
    }
    return [System.Text.Encoding]::UTF8.GetString($bytes)
}

function Get-HashManifestMap {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Manifest)
    $expected = New-Object 'System.Collections.Generic.Dictionary[string,string]' ([System.StringComparer]::Ordinal)
    foreach ($line in ($Manifest -split "`n")) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        if ($line -notmatch '^([0-9a-f]{64})  (.+)$') {
            throw 'The hash list from the previous job is malformed. Refusing to continue.'
        }
        $name = $Matches[2]
        if ($expected.ContainsKey($name)) {
            throw "The hash list repeats $name. Refusing to continue."
        }
        [void]$expected.Add($name, $Matches[1])
    }
    if ($expected.Count -lt 1) {
        throw 'The previous job sent no file hashes. Refusing to continue.'
    }
    return ,$expected
}

function Assert-FilesMatchManifest {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Manifest,
        [Parameter(Mandatory = $true)][System.IO.FileInfo[]]$Files
    )
    $expected = Get-HashManifestMap -Manifest $Manifest
    $actual = @($Files | Where-Object { $null -ne $_ })
    if ($actual.Count -ne $expected.Count) {
        throw "Refusing to continue. The previous job hashed $($expected.Count) files and this job has $($actual.Count)."
    }
    foreach ($file in $actual) {
        if (-not $expected.ContainsKey($file.Name)) {
            throw "Refusing to continue. $($file.Name) was not in the previous job's hash list."
        }
        $hash = Get-FileSha256Hex -Path $file.FullName
        if (-not [string]::Equals($hash, $expected[$file.Name], [StringComparison]::Ordinal)) {
            throw "Refusing to continue. $($file.Name) does not match the hash from the previous job."
        }
    }
}

function Assert-PinnedSevenZipMsi {
    param([Parameter(Mandatory = $true)][string]$Path)
    Assert-SevenZipDownloadPin
    $item = Get-Item -LiteralPath $Path
    $max = Get-SevenZipMsiMaxBytes
    if ($item.Length -lt 1 -or $item.Length -gt $max) {
        throw "The 7-Zip installer is $($item.Length) bytes. Refusing a file larger than $max bytes."
    }
    $hash = Get-FileSha256Hex -Path $Path
    if (-not [string]::Equals($hash, (Get-SevenZipMsiSha256), [StringComparison]::Ordinal)) {
        throw 'The 7-Zip installer hash does not match the pin. Refusing to extract it.'
    }
}

function Get-SigningSecretNames {
    return @(
        'WINDOWS_CERTIFICATE',
        'WINDOWS_CERTIFICATE_PASSWORD',
        'AZURE_TENANT_ID',
        'AZURE_CLIENT_ID',
        'AZURE_CLIENT_SECRET',
        'AZURE_CODE_SIGNING_ACCOUNT_NAME',
        'AZURE_CERT_PROFILE_NAME',
        'AZURE_ENDPOINT',
        'AZURE_DLIB_URL',
        'AZURE_DLIB_SHA256',
        'AZURE_DLIB_PATH'
    )
}

function Invoke-WithSigningSecretsHidden {
    param([Parameter(Mandatory = $true)][scriptblock]$Action)
    $saved = @{}
    foreach ($name in (Get-SigningSecretNames)) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $null)
    }
    try {
        & $Action
    } finally {
        foreach ($name in (Get-SigningSecretNames)) {
            [Environment]::SetEnvironmentVariable($name, $saved[$name])
        }
    }
}

function Get-NsisPluginAllowList {
    # Names and SHA-256 of the DLLs inside a Tauri NSIS setup: NSIS 3.11
    # Plugins/x86-unicode (System, nsDialogs, NSISdl, StartMenu) and
    # nsis_tauri_utils.dll SHA-1 75197fee3c6a814fe035788d1c34ead39349b860.
    $map = New-Object 'System.Collections.Generic.Dictionary[string,string]' ([System.StringComparer]::Ordinal)
    [void]$map.Add('System.dll', '8b4c47c4cf5e76ec57dd5a050d5acd832a0d532ee875d7b44f6cdaf68f90d37c')
    [void]$map.Add('nsDialogs.dll', 'b1350f487692057c8ffde75dcc55287a52a3272240d4d4912f24464b27551fc0')
    [void]$map.Add('nsis_tauri_utils.dll', '5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709')
    [void]$map.Add('StartMenu.dll', 'fe330320459637d8dd2b5d84d07925c38ec264152ffd3b1760a4d44d76536caa')
    [void]$map.Add('NSISdl.dll', 'ddf40229dd9d6b268902d8dea88c8a04aacf1af218dd29f6dcd35babc54ac08d')
    return ,$map
}

function Get-NsisUninstallBanner {
    return 'Nullsoft Install System v3.11'
}

function Get-NsisUninstallMinBytes {
    return 61440
}

function Get-NsisUninstallMaxBytes {
    return 204800
}

function Test-FileContainsAscii {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Needle
    )
    $hay = [System.IO.File]::ReadAllBytes($Path)
    $needleBytes = [System.Text.Encoding]::ASCII.GetBytes($Needle)
    if ($needleBytes.Length -eq 0 -or $hay.Length -lt $needleBytes.Length) {
        return $false
    }
    $limit = $hay.Length - $needleBytes.Length
    for ($i = 0; $i -le $limit; $i++) {
        $match = $true
        for ($j = 0; $j -lt $needleBytes.Length; $j++) {
            if ($hay[$i + $j] -ne $needleBytes[$j]) {
                $match = $false
                break
            }
        }
        if ($match) { return $true }
    }
    return $false
}

function Assert-SignerThumbprint {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Actual,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Expected
    )
    $right = $Expected.Trim().ToUpperInvariant()
    if ($right -notmatch '^[0-9A-F]{40}$') {
        throw 'SIGNER_THUMBPRINT is missing or not 40 hex characters. Refusing to sign.'
    }
    $left = $Actual.Trim().ToUpperInvariant()
    if ($left -notmatch '^[0-9A-F]{40}$') {
        throw 'The file has no signer thumbprint. Refusing to continue.'
    }
    if (-not [string]::Equals($left, $right, [StringComparison]::Ordinal)) {
        throw "The signer thumbprint $left is not the certificate from the binary sign job ($right)."
    }
}

function Assert-PayloadEntry {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Sha256,
        [Parameter(Mandatory = $true)][long]$ByteLength,
        [Parameter(Mandatory = $true)][ValidateSet('msi', 'nsis')][string]$Kind,
        [Parameter(Mandatory = $true)]$SignedHashes,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$SignerThumbprint,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$ActualThumbprint,
        [Parameter(Mandatory = $true)][bool]$BannerPresent
    )
    $hash = $Sha256.ToLowerInvariant()
    if ($SignedHashes.ContainsKey($Name)) {
        $expected = [string]$SignedHashes[$Name]
        if (-not [string]::Equals($hash, $expected, [StringComparison]::Ordinal)) {
            throw "Refusing to sign. $Name does not match the hash from the binary sign job."
        }
        Assert-SignerThumbprint -Actual $ActualThumbprint -Expected $SignerThumbprint
        return 'signed-binary'
    }
    $plugins = Get-NsisPluginAllowList
    if ($plugins.ContainsKey($Name)) {
        if (-not [string]::Equals($hash, [string]$plugins[$Name], [StringComparison]::Ordinal)) {
            throw "Refusing to sign. $Name is not the pinned NSIS plugin."
        }
        return 'plugin'
    }
    if ([string]::Equals($Name, 'uninstall.exe', [StringComparison]::Ordinal)) {
        if ($Kind -ne 'nsis') {
            throw 'Refusing to sign. uninstall.exe is not part of the MSI payload.'
        }
        $min = Get-NsisUninstallMinBytes
        $max = Get-NsisUninstallMaxBytes
        if ($ByteLength -lt $min -or $ByteLength -gt $max) {
            throw "Refusing to sign. uninstall.exe is $ByteLength bytes, outside $min..$max."
        }
        if (-not $BannerPresent) {
            throw 'Refusing to sign. uninstall.exe does not contain the NSIS 3.11 banner.'
        }
        return 'uninstall'
    }
    throw "Refusing to sign. $Name is an unexpected executable in the $Kind installer."
}

function Assert-InstallerPayload {
    param(
        [Parameter(Mandatory = $true)][string]$ExtractDirectory,
        [Parameter(Mandatory = $true)][ValidateSet('msi', 'nsis')][string]$Kind,
        [Parameter(Mandatory = $true)][string]$SignedManifest,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$SignerThumbprint,
        [Parameter(Mandatory = $true)][scriptblock]$ReadThumbprint
    )
    $signed = Get-HashManifestMap -Manifest $SignedManifest
    foreach ($required in (Get-RequiredExecutableNames)) {
        if (-not $signed.ContainsKey($required)) {
            throw "The binary sign job did not hash $required. Refusing to sign the installers."
        }
    }
    $binaries = @(Get-ChildItem -LiteralPath $ExtractDirectory -Recurse -File -ErrorAction SilentlyContinue | Where-Object {
        $null -ne $_ -and ($_.Extension.ToLowerInvariant() -eq '.exe' -or $_.Extension.ToLowerInvariant() -eq '.dll')
    })
    $seen = New-Object 'System.Collections.Generic.Dictionary[string,int]' ([System.StringComparer]::Ordinal)
    $uninstallCount = 0
    foreach ($file in $binaries) {
        $banner = $false
        if ([string]::Equals($file.Name, 'uninstall.exe', [StringComparison]::Ordinal)) {
            $uninstallCount++
            $min = [int64](Get-NsisUninstallMinBytes)
            $max = [int64](Get-NsisUninstallMaxBytes)
            if ($file.Length -ge $min -and $file.Length -le $max) {
                $hasVersion = Test-FileContainsAscii -Path $file.FullName -Needle (Get-NsisUninstallBanner)
                $hasHead = Test-FileContainsAscii -Path $file.FullName -Needle 'Nullsoft.NSIS.exehead'
                $banner = $hasVersion -and $hasHead
            }
        }
        $thumb = ''
        if ($signed.ContainsKey($file.Name)) {
            $thumb = [string](& $ReadThumbprint $file.FullName)
        }
        [void](Assert-PayloadEntry -Name $file.Name -Sha256 (Get-FileSha256Hex -Path $file.FullName) -ByteLength ([int64]$file.Length) -Kind $Kind -SignedHashes $signed -SignerThumbprint $SignerThumbprint -ActualThumbprint $thumb -BannerPresent $banner)
        if (-not $seen.ContainsKey($file.Name)) {
            $seen[$file.Name] = 0
        }
        $seen[$file.Name] = $seen[$file.Name] + 1
    }
    if ($Kind -eq 'nsis' -and $uninstallCount -ne 1) {
        throw "Refusing to sign. The NSIS installer must contain exactly one uninstall.exe (found $uninstallCount)."
    }
    if ($Kind -eq 'msi' -and $uninstallCount -ne 0) {
        throw 'Refusing to sign. The MSI extract must not contain uninstall.exe.'
    }
    foreach ($required in (Get-RequiredExecutableNames)) {
        $count = 0
        if ($seen.ContainsKey($required)) { $count = $seen[$required] }
        if ($count -lt 1) {
            throw "Refusing to sign. The $Kind extract does not contain $required."
        }
    }
}

function Write-GitHubOutputValue {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Value
    )
    if ([string]::IsNullOrWhiteSpace($env:GITHUB_OUTPUT)) { return }
    if ($Name -notmatch '^[A-Za-z0-9_-]+$' -or $Value.Contains("`n") -or $Value.Contains("`r")) {
        throw 'Refusing to write a GitHub output that is not a single line.'
    }
    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::AppendAllText($env:GITHUB_OUTPUT, "$Name=$Value`n", $utf8)
}

function Get-TopLevelExecutables {
    param([Parameter(Mandatory = $true)][string]$Directory)
    if (-not (Test-Path -LiteralPath $Directory)) {
        throw "No executables were found in $Directory."
    }
    $list = New-Object System.Collections.Generic.List[System.IO.FileInfo]
    $found = Get-ChildItem -LiteralPath $Directory -Filter *.exe -File -ErrorAction SilentlyContinue
    foreach ($file in @($found)) {
        if ($null -ne $file) { $list.Add($file) }
    }
    foreach ($file in $list) {
        Write-Output $file
    }
}

function Find-SignTool {
    # On GitHub Actions, ignore TAURI_WINDOWS_SIGNTOOL_PATH and PATH. A build
    # step must not be able to point signtool at a stand-in.
    $onActions = $env:GITHUB_ACTIONS -eq 'true'
    if (-not $onActions) {
        $override = $env:TAURI_WINDOWS_SIGNTOOL_PATH
        if (-not [string]::IsNullOrWhiteSpace($override)) {
            if (Test-Path -LiteralPath $override) {
                return (Resolve-Path -LiteralPath $override).Path
            }
            throw 'TAURI_WINDOWS_SIGNTOOL_PATH is set, and that file was not found.'
        }
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

function Get-AuthenticodeSignerThumbprint {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not $IsWindows) {
        throw 'Authenticode thumbprints are read on Windows only.'
    }
    $sig = Get-AuthenticodeSignature -LiteralPath $Path
    $cert = $null
    if ($null -ne $sig) { $cert = $sig.SignerCertificate }
    if ($null -eq $cert -or [string]::IsNullOrWhiteSpace([string]$cert.Thumbprint)) {
        throw "No Authenticode signer thumbprint on $(Split-Path -Leaf $Path)."
    }
    return ([string]$cert.Thumbprint).ToUpperInvariant()
}

function Select-AuthenticodeResultsForLabel {
    param(
        [Parameter(Mandatory = $true)][bool]$Signed,
        [object[]]$Results
    )
    $rows = @($Results | Where-Object { $null -ne $_ })
    if ($Signed) {
        foreach ($row in $rows) { Write-Output $row }
        return
    }
    foreach ($result in $rows) {
        $name = [string]$result.Name
        foreach ($requiredName in (Get-RequiredExecutableNames)) {
            if ([string]::Equals($name, $requiredName, [StringComparison]::Ordinal)) {
                Write-Output $result
                break
            }
        }
    }
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
        $parts.Add('SIGNING_ENABLED is true and this run is a v* tag push.')
        $parts.Add('cfw-zero-touch.exe and cfw-flash.exe were compiled with no certificate in the environment.')
        $parts.Add('A separate job checked the compile hashes, signed those executables, and did not run npm or cargo.')
        $parts.Add('The installers were bundled with no certificate. That job keeps a Rust toolchain because tauri bundle runs cargo metadata. It does not compile the app again.')
        $parts.Add('Before the installers were signed, the sign job extracted the MSI with msiexec /a and the setup executable with pinned 7-Zip. cfw-zero-touch.exe and cfw-flash.exe inside both matched the signed-binary hashes byte for byte, and their signer thumbprint matched that certificate. Unexpected executables and libraries were refused.')
        $parts.Add('Certificate material was read only in the sign jobs, from the release environment, and only after that inspection.')
        $parts.Add('The imported certificate was removed by thumbprint, and its key container was deleted.')
        if (-not [string]::IsNullOrWhiteSpace($VerifyNote)) { $parts.Add($VerifyNote) }
        $parts.Add('A tag push opens a draft GitHub release. You publish that draft from the releases page.')
    } else {
        $heading = 'This build is UNSIGNED.'
        $parts.Add('Signing stayed off for this run.')
        if ($SignStepOutcome -eq 'skipped') {
            $parts.Add('The Sign Windows binaries step was skipped. Tauri ran with no certificate in the environment.')
        } else {
            $parts.Add('Tauri ran with no certificate in the environment.')
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

The installers also carry `cfw-zero-touch.exe` (the app; the window title is CFW Card Studio) and `cfw-flash.exe` (the admin helper). Extract the MSI and check every `.exe` inside:

```bat
msiexec /a "the-setup.msi" /qn TARGETDIR=C:\cfw-msi-extract\
signtool verify /pa "C:\cfw-msi-extract\path\to\cfw-zero-touch.exe"
signtool verify /pa "C:\cfw-msi-extract\path\to\cfw-flash.exe"
```

Search `C:\cfw-msi-extract` for `cfw-zero-touch.exe` and `cfw-flash.exe` and run the same check. An unsigned build fails those two. A signed build passes them. The sign job also extracts the NSIS setup with pinned 7-Zip and checks that both files match the bytes that were signed, and that the signer thumbprint is the certificate from that job.

`SIGNING.txt` says either `unsigned` or `signed`.

## Hashes

```text
__SUMS__```
'@
    return $template.Replace('__HEADING__', $Heading).Replace('__BANNER__', $banner).Replace('__SUMS__', $SumsText)
}
