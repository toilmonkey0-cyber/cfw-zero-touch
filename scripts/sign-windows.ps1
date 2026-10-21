# Tauri signCommand entry point. The release workflow passes this script only
# when SIGNING_ENABLED is exactly true. Any other value refuses to sign.
# https://v2.tauri.app/distribute/sign/windows/

param(
    [Parameter(Position = 0)]
    [string]$File,
    [switch]$SelfCheck,
    [switch]$Prepare
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'release-lib.ps1')

function Import-PfxForSigning {
    $dir = Get-WorkDirectory
    $thumbPath = Join-Path $dir 'cfw-codesign.thumb'
    if (Test-Path -LiteralPath $thumbPath) {
        $cached = ([System.IO.File]::ReadAllText($thumbPath)).Trim()
        if (-not [string]::IsNullOrWhiteSpace($cached)) {
            return $cached
        }
    }

    $pfxPath = Join-Path $dir 'cfw-codesign.pfx'
    $b64 = ($env:WINDOWS_CERTIFICATE -replace '\s', '')
    try {
        $bytes = [Convert]::FromBase64String($b64)
    } catch {
        throw 'WINDOWS_CERTIFICATE is not valid base64. Refusing to sign.'
    }
    [System.IO.File]::WriteAllBytes($pfxPath, $bytes)
    $secure = ConvertTo-SecureString -String $env:WINDOWS_CERTIFICATE_PASSWORD -Force -AsPlainText
    try {
        $imported = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation 'Cert:\CurrentUser\My' -Password $secure -ErrorAction Stop
    } catch {
        throw 'The PFX could not be imported. Check WINDOWS_CERTIFICATE and WINDOWS_CERTIFICATE_PASSWORD on the release environment.'
    } finally {
        if (Test-Path -LiteralPath $pfxPath) {
            Remove-Item -LiteralPath $pfxPath -Force
        }
    }

    $leaf = @($imported) | Where-Object { $null -ne $_ -and $_.HasPrivateKey } | Select-Object -First 1
    if ($null -eq $leaf -or [string]::IsNullOrWhiteSpace($leaf.Thumbprint)) {
        throw 'The PFX did not contain a certificate with a private key.'
    }
    Write-Utf8Text -Path $thumbPath -Text $leaf.Thumbprint
    Write-Host "Imported code-signing certificate $($leaf.Thumbprint)"
    return $leaf.Thumbprint
}

$method = Get-SigningMethod
if ($method -eq 'off') {
    throw 'SIGNING_ENABLED is not true. Refusing to sign. This build stays unsigned.'
}
Assert-SigningMaterial -Method $method

if ($SelfCheck) {
    Write-Output "sign-windows: $method material is present. No file was signed."
    return
}

if ($Prepare) {
    if ($method -eq 'pfx') {
        [void](Import-PfxForSigning)
    }
    if ($method -eq 'azure') {
        if ([string]::IsNullOrWhiteSpace($env:AZURE_DLIB_PATH) -or -not (Test-Path -LiteralPath $env:AZURE_DLIB_PATH)) {
            throw 'AZURE_DLIB_PATH is missing. The build downloads the pinned dlib before signing.'
        }
    }
    Write-Output "sign-windows: $method is ready. No file was signed."
    return
}

if ([string]::IsNullOrWhiteSpace($File) -or -not (Test-Path -LiteralPath $File)) {
    throw 'sign-windows: no file to sign was provided.'
}

$tool = Find-SignTool
if ([string]::IsNullOrWhiteSpace($tool)) {
    throw 'signtool.exe was not found. Install the Windows SDK or set TAURI_WINDOWS_SIGNTOOL_PATH.'
}

if ($method -eq 'pfx') {
    $thumb = Import-PfxForSigning
    $timestamp = $env:WINDOWS_TIMESTAMP_URL
    if ([string]::IsNullOrWhiteSpace($timestamp)) {
        $timestamp = 'http://timestamp.digicert.com'
    }
    if ($timestamp -notmatch '^https?://') {
        throw 'WINDOWS_TIMESTAMP_URL must start with http:// or https://.'
    }
    Write-Output "sign-windows: signtool SHA256 timestamp $timestamp"
    & $tool sign /fd SHA256 /sha1 $thumb /tr $timestamp /td SHA256 /d 'CFW Card Studio' $File
    if ($LASTEXITCODE -ne 0) {
        throw "signtool sign failed for $File"
    }
    Write-Output "sign-windows: signed $File"
    return
}

$dlib = $env:AZURE_DLIB_PATH
if ([string]::IsNullOrWhiteSpace($dlib) -or -not (Test-Path -LiteralPath $dlib)) {
    throw 'AZURE_DLIB_PATH is missing. The build downloads the pinned dlib before signing.'
}
$endpoint = $env:AZURE_ENDPOINT.Trim()
if ($endpoint -notmatch '^https://') {
    throw 'AZURE_ENDPOINT must be an https URL.'
}
$meta = Join-Path (Get-WorkDirectory) 'cfw-azure-metadata.json'
$payload = [ordered]@{
    Endpoint                 = $endpoint
    CodeSigningAccountName   = $env:AZURE_CODE_SIGNING_ACCOUNT_NAME
    CertificateProfileName   = $env:AZURE_CERT_PROFILE_NAME
    ExcludeCredentials       = @(
        'ManagedIdentityCredential',
        'WorkloadIdentityCredential',
        'SharedTokenCacheCredential',
        'VisualStudioCredential',
        'VisualStudioCodeCredential',
        'AzureCliCredential',
        'AzurePowerShellCredential',
        'AzureDeveloperCliCredential',
        'InteractiveBrowserCredential'
    )
}
Write-Utf8Text -Path $meta -Text ($payload | ConvertTo-Json -Depth 6)
Write-Output 'sign-windows: signtool SHA256 with Azure Artifact Signing'
& $tool sign /fd SHA256 /tr 'http://timestamp.acs.microsoft.com' /td SHA256 /dlib $dlib /dmdf $meta $File
if ($LASTEXITCODE -ne 0) {
    throw "signtool sign failed for $File"
}
Write-Output "sign-windows: signed $File"
