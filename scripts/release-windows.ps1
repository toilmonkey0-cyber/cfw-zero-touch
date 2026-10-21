# Build the Windows installer, or run the release-scaffold self-test.
# Signing stays off unless SIGNING_ENABLED is exactly true.

param(
    [ValidateSet('build', 'self-test')]
    [string]$Mode = 'build'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'release-lib.ps1')

$Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path

function Get-AuthenticodeResults {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)][string]$SignTool
    )
    $results = New-Object System.Collections.Generic.List[object]
    $found = Get-ChildItem -LiteralPath $Directory -File -ErrorAction SilentlyContinue
    foreach ($file in @($found)) {
        if ($null -eq $file) { continue }
        $ext = $file.Extension.ToLowerInvariant()
        if ($ext -ne '.exe' -and $ext -ne '.msi') { continue }
        & $SignTool verify /pa $file.FullName
        $results.Add([pscustomobject]@{
            Name     = $file.Name
            ExitCode = [int]$LASTEXITCODE
        })
    }
    foreach ($result in $results) {
        Write-Output $result
    }
}

function Invoke-ReleaseBuild {
    Set-Location -LiteralPath $Root
    $version = Get-AppVersion -Root $Root
    Write-Output "Application version $version"
    if ($env:GITHUB_REF_TYPE -eq 'tag') {
        Assert-TagMatchesVersion -Tag $env:GITHUB_REF_NAME -Version $version
    }

    $method = Get-SigningMethod
    $signed = $method -ne 'off'
    if (-not $signed) {
        $outcome = $env:SIGN_STEP_OUTCOME
        if (-not [string]::IsNullOrWhiteSpace($outcome) -and $outcome -ne 'skipped') {
            throw "The unsigned job's Sign Windows binaries step was '$outcome', not skipped."
        }
    }

    if ($signed) {
        Assert-SigningMaterial -Method $method
        & (Join-Path $PSScriptRoot 'sign-windows.ps1') -SelfCheck
        if ($method -eq 'azure') {
            Install-AzureDlib
        }
        & (Join-Path $PSScriptRoot 'sign-windows.ps1') -Prepare
        $configPath = Join-Path (Get-WorkDirectory) 'tauri-signing.json'
        $scriptPath = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot 'sign-windows.ps1')).Path
        [void](New-TauriSigningConfigJson -ScriptPath $scriptPath -Destination $configPath)
        Write-Output "Tauri signCommand is scripts/sign-windows.ps1 ($method)."
        & npm run tauri build -- --config $configPath
        if ($LASTEXITCODE -ne 0) {
            throw "tauri build exited $LASTEXITCODE"
        }
    } else {
        Write-Output 'SIGNING_ENABLED is not true. Building without a signCommand. This build is UNSIGNED.'
        & npm run tauri build
        if ($LASTEXITCODE -ne 0) {
            throw "tauri build exited $LASTEXITCODE"
        }
    }

    $bundle = Join-Path $Root 'src-tauri/target/release/bundle'
    $destination = Join-Path $Root 'release'
    Copy-ReleaseFiles -BundleRoot $bundle -Destination $destination -Signed $signed

    $tool = Find-SignTool
    $onActions = $env:GITHUB_ACTIONS -eq 'true'
    $verifyNote = ''
    if ([string]::IsNullOrWhiteSpace($tool)) {
        Assert-AuthenticodeResults -Signed $signed -FoundSignTool $false -RunningOnActions $onActions -Results @()
        $verifyNote = 'signtool.exe was not on this machine, so Authenticode was not checked here.'
    } else {
        $results = @(Get-AuthenticodeResults -Directory $destination -SignTool $tool)
        Assert-AuthenticodeResults -Signed $signed -FoundSignTool $true -RunningOnActions $onActions -Results $results
        if ($signed) {
            $verifyNote = 'signtool verify /pa passed for every shipped .exe and .msi.'
        } else {
            $verifyNote = 'signtool verify /pa failed for the shipped installers, which is what an unsigned build should do.'
        }
    }

    $sums = [System.IO.File]::ReadAllText((Join-Path $destination 'SHA256SUMS'))
    $heading = "CFW Card Studio $version"
    if ($env:GITHUB_REF_TYPE -eq 'tag' -and -not [string]::IsNullOrWhiteSpace($env:GITHUB_REF_NAME)) {
        $heading = "CFW Card Studio $($env:GITHUB_REF_NAME)"
    }
    $notes = New-ReleaseNotes -Heading $heading -Signed $signed -SumsText $sums
    Write-Utf8Text -Path (Join-Path $destination 'RELEASE_NOTES.md') -Text $notes

    $names = New-Object System.Collections.Generic.List[string]
    foreach ($file in @(Get-ChildItem -LiteralPath $destination -File | Sort-Object Name)) {
        if ($null -ne $file) { $names.Add($file.Name) }
    }
    $summary = New-ReleaseSummary -Signed $signed -FileNames $names.ToArray() -VerifyNote $verifyNote -SignStepOutcome $env:SIGN_STEP_OUTCOME
    Write-Output $summary
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_STEP_SUMMARY)) {
        Add-Utf8Text -Path $env:GITHUB_STEP_SUMMARY -Text ("`n" + $summary + "`n")
    }
}

function Assert-That {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw "self-test failed: $Message"
    }
}

function Invoke-SignChild {
    param([hashtable]$Variables, [string[]]$ExtraArgs)
    $keys = @(
        'SIGNING_ENABLED',
        'SIGNING_METHOD',
        'WINDOWS_CERTIFICATE',
        'WINDOWS_CERTIFICATE_PASSWORD',
        'WINDOWS_TIMESTAMP_URL',
        'AZURE_TENANT_ID',
        'AZURE_CLIENT_ID',
        'AZURE_CLIENT_SECRET',
        'AZURE_CODE_SIGNING_ACCOUNT_NAME',
        'AZURE_CERT_PROFILE_NAME',
        'AZURE_ENDPOINT',
        'AZURE_DLIB_URL',
        'AZURE_DLIB_SHA256',
        'AZURE_DLIB_PATH',
        'TAURI_WINDOWS_SIGNTOOL_PATH',
        'SIGN_STEP_OUTCOME'
    )
    $saved = @{}
    foreach ($key in $keys) {
        $saved[$key] = [Environment]::GetEnvironmentVariable($key)
        $value = $null
        if ($Variables.ContainsKey($key)) {
            $value = [string]$Variables[$key]
        }
        [Environment]::SetEnvironmentVariable($key, $value)
    }
    $script = Join-Path $PSScriptRoot 'sign-windows.ps1'
    try {
        $captured = & pwsh -NoProfile -ExecutionPolicy Bypass -File $script @ExtraArgs 2>&1
        $code = $LASTEXITCODE
        $text = @($captured | ForEach-Object { "$_" }) -join "`n"
        return @{ Code = $code; Text = $text }
    } finally {
        foreach ($key in $keys) {
            [Environment]::SetEnvironmentVariable($key, $saved[$key])
        }
    }
}

function Assert-ReleaseWorkflowShape {
    param([string]$Path)
    $lines = @(Get-Content -LiteralPath $Path)
    $inJobs = $false
    $job = ''
    $secretCount = 0
    $writeCount = 0
    $environmentCount = 0
    foreach ($line in $lines) {
        if ($line -eq 'jobs:') {
            $inJobs = $true
            continue
        }
        if ($inJobs -and $line -match '^[^ \t#]') {
            $inJobs = $false
        }
        if ($inJobs -and $line -match '^  ([a-z0-9-]+):\s*$') {
            $job = $Matches[1]
        }
        if ($line -match 'secrets\.') {
            if ($job -ne 'signed') {
                throw "self-test failed: signing secret referenced outside the signed job: $line"
            }
            $secretCount++
        }
        if ($line -match 'contents:\s*write') {
            if ($job -ne 'release') {
                throw "self-test failed: contents write is only for the release job: $line"
            }
            $writeCount++
        }
        if ($line -match 'environment:\s*release\s*$') {
            if ($job -ne 'signed') {
                throw "self-test failed: the release environment is only for the signed job: $line"
            }
            $environmentCount++
        }
    }
    Assert-That ($secretCount -ge 1) 'the signed job should reference signing secrets'
    Assert-That ($writeCount -eq 1) 'contents: write should appear once'
    Assert-That ($environmentCount -eq 1) 'environment: release should appear once'
    $text = [System.IO.File]::ReadAllText($Path)
    Assert-That (-not ($text -match 'pull_request')) 'the release workflow must not mention a pull request trigger'
    Assert-That ($text -match '--draft') 'the release command must pass --draft'
    Assert-That ($text -match '--verify-tag') 'the release command must pass --verify-tag'
    Assert-That ($text -match '--latest=false') 'the release command must pass --latest=false'
    Assert-That (-not ($text -match '--prerelease')) 'the release command must not mark a prerelease'
    Assert-That ($text -match 'dry_run') 'workflow_dispatch should expose dry_run'
    Assert-That ($text -match "SIGNING_ENABLED: 'false'") 'the unsigned job should force signing off'
}

function Invoke-ReleaseSelfTest {
    $checks = 0

    $version = Get-AppVersion -Root $Root
    Assert-That (-not [string]::IsNullOrWhiteSpace($version)) 'version should be readable'
    Assert-TagMatchesVersion -Tag "v$version" -Version $version
    $tagThrew = $false
    try { Assert-TagMatchesVersion -Tag 'v0.0.0-nope' -Version $version } catch { $tagThrew = $true }
    Assert-That $tagThrew 'a mismatched tag should be refused'
    $checks++

    $temp = Join-Path ([System.IO.Path]::GetTempPath()) ("cfw-release-selftest-" + [guid]::NewGuid().ToString('n'))
    New-Item -ItemType Directory -Path $temp | Out-Null
    try {
        $bundle = Join-Path $temp 'bundle'
        New-Item -ItemType Directory -Path (Join-Path $bundle 'msi') | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $bundle 'nsis') | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $bundle 'secrets') | Out-Null
        $exeName = 'CFW Card Studio_0.2.0_x64-setup.exe'
        $msiName = 'CFW Card Studio_0.2.0_x64_en-US.msi'
        [System.IO.File]::WriteAllBytes((Join-Path (Join-Path $bundle 'nsis') $exeName), [Text.Encoding]::ASCII.GetBytes('abc'))
        [System.IO.File]::WriteAllBytes((Join-Path (Join-Path $bundle 'msi') $msiName), [Text.Encoding]::ASCII.GetBytes('abc'))
        [System.IO.File]::WriteAllBytes((Join-Path (Join-Path $bundle 'secrets') 'cert.pfx'), [Text.Encoding]::ASCII.GetBytes('secret'))
        [System.IO.File]::WriteAllBytes((Join-Path (Join-Path $bundle 'nsis') 'extra.dll'), [Text.Encoding]::ASCII.GetBytes('dll'))
        Set-Content -LiteralPath (Join-Path (Join-Path $bundle 'msi') 'notes.txt') -Value 'nope' -Encoding ascii

        $emptyThrew = $false
        try {
            Copy-ReleaseFiles -BundleRoot (Join-Path $temp 'missing') -Destination (Join-Path $temp 'empty-out') -Signed $false
        } catch { $emptyThrew = $true }
        Assert-That $emptyThrew 'a bundle without installers should be refused'

        $out = Join-Path $temp 'release'
        Copy-ReleaseFiles -BundleRoot $bundle -Destination $out -Signed $false
        $shipped = @(Get-ChildItem -LiteralPath $out -File | ForEach-Object { $_.Name })
        Assert-That ($shipped -contains $exeName) 'setup exe should be staged'
        Assert-That ($shipped -contains $msiName) 'msi should be staged'
        Assert-That ($shipped -contains 'SIGNING.txt') 'SIGNING.txt should be staged'
        Assert-That ($shipped -contains 'SHA256SUMS') 'SHA256SUMS should be staged'
        Assert-That ($shipped -notcontains 'cert.pfx') 'a pfx must not be staged'
        Assert-That ($shipped -notcontains 'extra.dll') 'a loose dll must not be staged'
        Assert-That ($shipped -notcontains 'notes.txt') 'a text file must not be staged'

        $labelBytes = [System.IO.File]::ReadAllBytes((Join-Path $out 'SIGNING.txt'))
        $labelText = [Text.Encoding]::UTF8.GetString($labelBytes)
        Assert-That ($labelText -eq "unsigned`n") 'SIGNING.txt should say unsigned'
        Assert-That (-not ($labelBytes -contains 13)) 'SIGNING.txt should use LF'

        $sumBytes = [System.IO.File]::ReadAllBytes((Join-Path $out 'SHA256SUMS'))
        Assert-That (-not ($sumBytes -contains 13)) 'SHA256SUMS should use LF'
        Assert-That ($sumBytes[$sumBytes.Length - 1] -eq 10) 'SHA256SUMS should end with LF'
        $sumText = [Text.Encoding]::UTF8.GetString($sumBytes)
        $known = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
        Assert-That ($sumText.Contains("$known  $exeName")) 'exe hash should be the SHA-256 of abc'
        Assert-That ($sumText.Contains("$known  $msiName")) 'msi hash should be the SHA-256 of abc'
        $signingLine = @($sumText -split "`n" | Where-Object { $_ -match 'SIGNING.txt$' })
        Assert-That ($signingLine.Count -eq 1) 'SIGNING.txt should be hashed once'
        Assert-That ($sumText -notmatch 'SHA256SUMS') 'SHA256SUMS should not hash itself'
        Assert-That ($sumText -notmatch 'cert\.pfx') 'the pfx name should not appear in the sums'
        Write-Utf8Text -Path (Join-Path $out 'RELEASE_NOTES.md') -Text 'notes'
        Write-Sha256Sums -Directory $out
        $rehashed = [Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes((Join-Path $out 'SHA256SUMS')))
        Assert-That ($rehashed -notmatch 'RELEASE_NOTES') 'release notes stay out of SHA256SUMS'
        Assert-That ($rehashed.Contains("$known  $exeName")) 'rehash should keep the installer lines'

        $signedOut = Join-Path $temp 'release-signed'
        Copy-ReleaseFiles -BundleRoot $bundle -Destination $signedOut -Signed $true
        $signedLabel = [Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes((Join-Path $signedOut 'SIGNING.txt')))
        Assert-That ($signedLabel -eq "signed`n") 'SIGNING.txt should say signed'

        $notes = New-ReleaseNotes -Heading 'CFW Card Studio v0.2.0' -Signed $false -SumsText $sumText
        Assert-That ($notes.Contains('This build is UNSIGNED')) 'unsigned notes should say UNSIGNED'
        Assert-That ($notes.Contains('signtool verify /pa')) 'notes should show how to verify'
        Assert-That ($notes.Contains('sha256sum -c SHA256SUMS')) 'notes should show the checksum check'
        Assert-That ($notes.Contains($known)) 'notes should include the hashes'
        $signedNotes = New-ReleaseNotes -Heading 'CFW Card Studio v0.2.0' -Signed $true -SumsText $sumText
        Assert-That ($signedNotes.Contains('This build is SIGNED')) 'signed notes should say SIGNED'
        $checks++

        $configPath = Join-Path $temp 'signing.json'
        $json = New-TauriSigningConfigJson -ScriptPath 'C:\tools\sign-windows.ps1' -Destination $configPath
        $parsed = $json | ConvertFrom-Json
        $signArgs = @($parsed.bundle.windows.signCommand.args)
        Assert-That ($parsed.bundle.windows.signCommand.cmd -eq 'powershell.exe') 'signCommand cmd should be powershell.exe'
        Assert-That ($signArgs -contains '%1') 'signCommand should pass %1 for the file Tauri wants signed'
        Assert-That ($signArgs -contains 'C:\tools\sign-windows.ps1') 'signCommand should point at the script'
        $checks++

        $unsignedSummary = New-ReleaseSummary -Signed $false -FileNames @($exeName) -VerifyNote 'signtool verify /pa failed for the shipped installers, which is what an unsigned build should do.' -SignStepOutcome 'skipped'
        Assert-That ($unsignedSummary.Contains('This build is UNSIGNED.')) 'summary should say UNSIGNED'
        Assert-That ($unsignedSummary.Contains('The Sign Windows binaries step was skipped.')) 'summary should say the sign step was skipped'
        $signedSummary = New-ReleaseSummary -Signed $true -FileNames @($exeName) -VerifyNote 'signtool verify /pa passed for every shipped .exe and .msi.' -SignStepOutcome ''
        Assert-That ($signedSummary.Contains('This build is SIGNED.')) 'summary should say SIGNED'
        $checks++

        $authThrew = $false
        try {
            Assert-AuthenticodeResults -Signed $false -FoundSignTool $true -RunningOnActions $true -Results @([pscustomobject]@{ Name = 'app.exe'; ExitCode = 0 })
        } catch { $authThrew = $true }
        Assert-That $authThrew 'a verify success should refuse the unsigned label'
        $ok = Assert-AuthenticodeResults -Signed $false -FoundSignTool $true -RunningOnActions $true -Results @([pscustomobject]@{ Name = 'app.exe'; ExitCode = 1 })
        Assert-That ($ok -eq 'checked') 'a verify failure should keep the unsigned label'
        $signedOk = Assert-AuthenticodeResults -Signed $true -FoundSignTool $true -RunningOnActions $true -Results @([pscustomobject]@{ Name = 'app.exe'; ExitCode = 0 })
        Assert-That ($signedOk -eq 'checked') 'a verify success should accept the signed label'
        $signedThrew = $false
        try {
            Assert-AuthenticodeResults -Signed $true -FoundSignTool $true -RunningOnActions $true -Results @([pscustomobject]@{ Name = 'app.exe'; ExitCode = 1 })
        } catch { $signedThrew = $true }
        Assert-That $signedThrew 'a verify failure should refuse the signed label'
        $missingActions = $false
        try {
            Assert-AuthenticodeResults -Signed $false -FoundSignTool $false -RunningOnActions $true -Results @()
        } catch { $missingActions = $true }
        Assert-That $missingActions 'Actions should fail when signtool is missing'
        $localSkip = Assert-AuthenticodeResults -Signed $false -FoundSignTool $false -RunningOnActions $false -Results @()
        Assert-That ($localSkip -eq 'skipped') 'a local machine without signtool can skip the check'
        $checks++

        $httpThrew = $false
        try { Assert-AzureDlibPin -Url 'http://example.com/dlib.zip' -Hash ('ab' * 32) } catch { $httpThrew = $true }
        Assert-That $httpThrew 'an http dlib URL should be refused'
        $shortThrew = $false
        try { Assert-AzureDlibPin -Url 'https://example.com/dlib.zip' -Hash 'abcd' } catch { $shortThrew = $true }
        Assert-That $shortThrew 'a short dlib hash should be refused'
        Assert-AzureDlibPin -Url 'https://example.com/dlib.zip' -Hash ('ab' * 32)
        $checks++
    } finally {
        if (Test-Path -LiteralPath $temp) {
            Remove-Item -LiteralPath $temp -Recurse -Force
        }
    }

    $off = Invoke-SignChild -Variables @{} -ExtraArgs @('-SelfCheck')
    Assert-That ($off.Code -ne 0) 'signing should fail when the switch is off'
    Assert-That ($off.Text -match 'unsigned') 'the refusal should say the build stays unsigned'

    $upper = Invoke-SignChild -Variables @{ SIGNING_ENABLED = 'TRUE' } -ExtraArgs @('-SelfCheck')
    Assert-That ($upper.Code -ne 0) 'TRUE is not the on switch'

    $marker = 'super-secret-marker-do-not-print'
    $missingPfx = Invoke-SignChild -Variables @{
        SIGNING_ENABLED             = 'true'
        WINDOWS_CERTIFICATE_PASSWORD = $marker
    } -ExtraArgs @('-SelfCheck')
    Assert-That ($missingPfx.Code -ne 0) 'pfx signing should fail closed without a certificate'
    Assert-That ($missingPfx.Text -match 'WINDOWS_CERTIFICATE') 'the pfx error should name the empty secret'
    Assert-That (-not ($missingPfx.Text.Contains($marker))) 'the password must not be printed'

    $ready = Invoke-SignChild -Variables @{
        SIGNING_ENABLED              = 'true'
        SIGNING_METHOD               = 'pfx'
        WINDOWS_CERTIFICATE          = 'AAAA'
        WINDOWS_CERTIFICATE_PASSWORD = $marker
    } -ExtraArgs @('-SelfCheck')
    Assert-That ($ready.Code -eq 0) 'self-check should pass when pfx material is present'
    Assert-That ($ready.Text -match 'No file was signed') 'self-check must not sign a file'
    Assert-That (-not ($ready.Text.Contains($marker))) 'self-check must not print the password'

    $badPrepare = Invoke-SignChild -Variables @{
        SIGNING_ENABLED              = 'true'
        SIGNING_METHOD               = 'pfx'
        WINDOWS_CERTIFICATE          = 'AAAA'
        WINDOWS_CERTIFICATE_PASSWORD = $marker
    } -ExtraArgs @('-Prepare')
    Assert-That ($badPrepare.Code -ne 0) 'a dummy PFX should fail before any file is signed'
    Assert-That (-not ($badPrepare.Text.Contains($marker))) 'PFX import errors must not print the password'

    $badMethod = Invoke-SignChild -Variables @{
        SIGNING_ENABLED = 'true'
        SIGNING_METHOD  = 'Azure'
    } -ExtraArgs @('-SelfCheck')
    Assert-That ($badMethod.Code -ne 0) 'SIGNING_METHOD is case-sensitive'
    Assert-That ($badMethod.Text -match 'empty, pfx, or azure') 'the method error should name the allowed values'
    Assert-That (-not ($badMethod.Text.Contains('Azure'))) 'a rejected method value should not be echoed'

    $missingAzure = Invoke-SignChild -Variables @{
        SIGNING_ENABLED = 'true'
        SIGNING_METHOD  = 'azure'
    } -ExtraArgs @('-SelfCheck')
    Assert-That ($missingAzure.Code -ne 0) 'azure signing should fail closed without secrets'
    Assert-That ($missingAzure.Text -match 'AZURE_TENANT_ID') 'the azure error should name the empty values'
    Assert-That ($missingAzure.Text -match 'AZURE_DLIB_SHA256') 'the azure error should require the dlib pin'
    $checks++

    Assert-ReleaseWorkflowShape -Path (Join-Path $Root '.github/workflows/release.yml')
    $ci = [System.IO.File]::ReadAllText((Join-Path $Root '.github/workflows/ci.yml'))
    Assert-That (-not ($ci -match 'WINDOWS_CERTIFICATE')) 'the pull-request CI workflow must not name the signing certificate'
    Assert-That (-not ($ci -match 'AZURE_CLIENT_SECRET')) 'the pull-request CI workflow must not name the Azure client secret'
    Assert-That (-not ($ci -match '\{\{\s*secrets\.')) 'the pull-request CI workflow must not reference Actions secrets'
    $checks++

    Write-Output "release scaffold self-test passed ($checks groups)"
}

if ($Mode -eq 'self-test') {
    Invoke-ReleaseSelfTest
    return
}

Invoke-ReleaseBuild
