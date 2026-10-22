# Cut a release

A version tag `v*` builds the Windows installer, writes `SHA256SUMS`, and opens a **draft** GitHub release with those files attached. You publish the draft yourself from the releases page. The workflow has no publish step.

Signing stays off until you set the repository variable `SIGNING_ENABLED` to exactly `true`. Until then the job summary, `SIGNING.txt`, and the draft all say **UNSIGNED**.

Nothing in this document buys a certificate or starts a paid account.

## Before you tag

The installer version and the copy shown in the app have to be the same number. These four have to match:

- `package.json` `version`
- `package-lock.json` (the top-level `version`, and the version on the `""` package)
- `src-tauri/Cargo.toml` `version` (this is the version the app shows)
- `src-tauri/tauri.conf.json` `version` (this is the version on the installer)

The tag is `v` plus that number. For `0.2.0` the tag is `v0.2.0`. The workflow stops if the tag and those files disagree.

`profiles-feed.json` has its own `app.version`. That is the number the in-app "new build available" banner compares against. Bump it in the same commit when you want the banner to mention this release. The release workflow leaves the feed alone.

Merge that commit to `main` and wait until CI is green.

## Rehearse with a dry run

The dry run builds the installer and uploads `SHA256SUMS` as an Actions artifact. It does not open a release.

1. Open the repository on GitHub.
2. Click **Actions**.
3. Click **Release** in the left sidebar.
4. Click **Run workflow**.
5. Choose the branch you want to build.
6. Leave **dry_run** checked.
7. Click **Run workflow**.

You need write access to start it. When it finishes, open the run and download the `windows-release` artifact. It contains the `.msi`, the setup `.exe`, `SHA256SUMS`, `SIGNING.txt`, and `RELEASE_NOTES.md`.

A dispatch always runs **Windows installer (unsigned)**, including when `SIGNING_ENABLED` is already `true`. The compile job also requires a push of a `v*` tag, so **Run workflow** never asks the `release` environment for a certificate. The summary says **This build is UNSIGNED.** The step **Sign Windows binaries** is skipped.

Clearing **dry_run** fails the run on purpose. A dispatch is only a rehearsal. Push a tag when you want a draft.

### Why a pull request cannot run this dry run

GitHub starts **Run workflow** for someone with write access. A pull request does not start it.

This workflow also has no pull-request trigger. A pull request runs the workflow file from the pull-request branch. That branch could be edited to print secrets after an environment approval. Signing secrets are named only in the two sign jobs, and a pull request never starts those jobs.

The pull-request check `windows-installer` in `.github/workflows/ci.yml` still builds an unsigned installer. That artifact is separate from a release. The `frontend` job runs `scripts/release-windows.ps1 -Mode self-test`, which checks the checksum file, the unsigned label, and the rules in the release workflow.

## Cut the release

On the commit you want to ship, usually current `main` after the version bump:

```bash
git checkout main
git pull
git tag v0.2.0
git push origin v0.2.0
```

Use the real version in place of `0.2.0`. Pushing the tag starts the workflow. The workflow refuses to create a tag itself (`--verify-tag`).

When the run is green:

1. Open **Releases**.
2. Open the draft. The title ends in `(unsigned)` or `(signed)`.
3. Read the banner at the top of the notes.
4. Download the files and check them with the steps below.
5. Click **Publish release** if you want other people to see it.

A draft is visible to people who can edit the repository. It becomes the public latest release when you publish it. The workflow checks afterwards that the release is still a draft, and it passes `--latest=false` so a mistake here does not mark the release as latest.

If a release for that tag already exists, the job stops and leaves it alone. Delete the draft yourself before you re-run the tag, and only if you mean to replace it.

## Signing stays off

`SIGNING_ENABLED` is a repository variable (**Settings → Secrets and variables → Actions → Variables**). Leave it unset. The only value that turns signing on is the exact lowercase word `true`. `TRUE`, `yes`, and `1` keep the build unsigned.

While it is off, and also on every **Run workflow** dispatch:

- The installer is compiled with no certificate in the environment.
- The **Sign Windows binaries** step is skipped.
- No certificate secret is read.
- `SIGNING.txt` contains `unsigned`.
- `signtool verify /pa` fails on the setup `.exe` and the `.msi`, and the job summary says so.
- The job extracts the MSI with `msiexec /a` and runs `signtool verify /pa` on every `.exe` inside. Those fail too, which is what an unsigned build should do. The extract has to contain `cfw-zero-touch.exe` and `cfw-flash.exe`.

## Turn signing on later

Do this after you have picked a vendor and read the current price yourself. **VERIFY** every cost, region, and eligibility rule on the vendor's own page. Prices move. This repository does not subscribe to Azure, does not buy a certificate, and does not store a card.

Create the protected environment **before** you set `SIGNING_ENABLED`, and **before** you add any signing secret.

### Create the `release` environment

These switches live on the GitHub website. A workflow file cannot turn them on.

1. Open the repository **Settings**.
2. In the left sidebar, under **Secrets and variables**, click **Actions**, or open **Environments** if your page shows that first.
3. Click **New environment**.
4. Name it `release`. The workflow looks for that exact name.
5. Turn on **Required reviewers** and add at least one person. A signed build waits until that person approves it.
6. Under **Deployment branches and tags**, choose selected tags and add the pattern `v*`. A branch dry run then cannot use the signing secrets.
7. Save.

If a sign job has already run, GitHub may have created `release` with none of these rules. Delete that environment and create it again with the reviewers and the tag rule before you add secrets.

Put signing secrets on this environment. Leave **Repository secrets** empty of certificates, passwords, and Azure client secrets. A repository secret can be read by any workflow job that names it. An environment secret is handed only to a job that names the environment. Keep them at environment level.

### Limit who can push a v* tag

The compile job runs when someone pushes a tag named `v*` and signing is on. The two sign jobs then wait on the `release` environment. Add a tag ruleset so only admins can create or move those tags. Do this on the GitHub website. This repository change does not edit those settings for you.

1. Open the repository **Settings**.
2. Open **Rules**, then **Rulesets**.
3. Click **New ruleset** and choose **Tag**.
4. Name it `release-tags`.
5. Under target tags, include the pattern `v*`.
6. Enable **Restrict creations** and **Restrict updates**. Allow only repository admins, or an admin team you already use. Leave the bypass list empty.
7. Enable **Restrict deletions** the same way, so a tag cannot be removed or pointed at a different commit quietly.
8. Save.

A person who can push any tag can otherwise point `v*` at a commit that changes this workflow. The ruleset is what stops that. The environment reviewers are the second check, and they only see a signed build after the tag exists.

### Azure Artifact Signing, formerly Trusted Signing

Azure Artifact Signing is Microsoft's cloud code-signing service. The private key stays in Microsoft's hardware. You sign from GitHub Actions with `signtool` and Microsoft's `Azure.CodeSigning.Dlib.dll`. There is no `.pfx` file to copy around.

**VERIFY** before anyone pays:

- The current price, on the Azure Artifact Signing product page. Microsoft's Windows code-signing comparison has quoted a monthly fee. Read the live page. Do not start a subscription from this document.
- Whether your country and account type can enroll. Microsoft has limited this by region, and the list changes.
- How long identity validation takes. Plan on days, not the same afternoon.
- The regional endpoint shown on your account. A public example Microsoft has used for East US is `https://eus.codesigning.azure.net`. Copy the endpoint from the portal. Guessing the wrong region fails the build.
- The timestamp URL. The scaffold uses `http://timestamp.acs.microsoft.com`, which is the timestamp server Microsoft documents for this service. Confirm it is still the one they recommend. Artifact Signing certificates are short-lived, so a signature without a timestamp dies with the certificate.

When you are ready, set repository **variables** (these are pins, not secrets):

| Variable | Value |
| --- | --- |
| `SIGNING_METHOD` | `azure` |
| `AZURE_DLIB_URL` | An `https://` URL for the dlib zip or the `Azure.CodeSigning.Dlib.dll` file |
| `AZURE_DLIB_SHA256` | The 64-character SHA-256 of that exact download |

The URL is visible to people who can read Actions logs. Use a public Microsoft download. Leave tokens and shared-access signatures out of it. The workflow downloads at most 50 MB, checks the hash, and looks for `Azure.CodeSigning.Dlib.dll` (the copy under an `x64` folder when the zip has more than one). A hash mismatch stops the build.

Set environment **secrets** on `release`:

| Secret | What it is |
| --- | --- |
| `AZURE_TENANT_ID` | Directory (tenant) id of the app registration |
| `AZURE_CLIENT_ID` | Application (client) id |
| `AZURE_CLIENT_SECRET` | Client secret for that app. Rotate it. A later improvement is a federated credential so this secret can go away. **VERIFY** Azure's current GitHub OIDC instructions before you switch. |
| `AZURE_CODE_SIGNING_ACCOUNT_NAME` | Artifact Signing account name |
| `AZURE_CERT_PROFILE_NAME` | Certificate profile name |
| `AZURE_ENDPOINT` | The regional `https://` endpoint from the portal |

Each sign job writes a metadata file on the runner from the account name, profile, and endpoint. The client secret stays in that job's environment for `signtool`. It is not written into the repo, and it is not written into that metadata file. The compile job and the bundle job have no certificate variables, and they do not use the `release` environment.

The binary sign job calls `signtool sign /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 /dlib <dlib> /dmdf <metadata>` on `cfw-zero-touch.exe` and `cfw-flash.exe`. After the installers are bundled, the installer sign job runs the same command on the finished setup `.exe` and the `.msi`.

Set `SIGNING_ENABLED` to `true` last. The next `v*` tag compiles with no certificate, waits for a reviewer, signs the executables, bundles, waits for a reviewer again, and signs the installers. If any of those values is missing, the sign job fails and does not upload an installer labelled signed.

### An OV or EV certificate

An Organization Validation (OV) or Extended Validation (EV) certificate comes from a public certificate authority such as DigiCert, Sectigo, or SSL.com.

**VERIFY** before anyone pays:

- The current annual price on that CA's page. Published comparisons quote an annual fee that is higher for EV than for OV. Read the CA's page.
- Where the private key lives. Since June 2023, new code-signing keys are supposed to stay on a hardware token or a cloud HSM. A CA may not give you a `.pfx` you can download. Ask them before you pay.
- What SmartScreen does today. EV used to be bought so the first download skipped SmartScreen. Confirm that is still true before you pay extra for EV. Microsoft's current comparison says reputation now builds over time for these certificates too. **VERIFY** that claim on the live page.

If the CA gives you a `.pfx`, the scaffold already signs with it. Leave `SIGNING_METHOD` empty or set it to `pfx`.

On your own machine, turn the file into one line of base64 and copy it to the clipboard. Do not commit the file or the text.

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes('C:\path\codesign.pfx')) | Set-Clipboard
```

Put these secrets on the `release` environment:

| Secret | What it is |
| --- | --- |
| `WINDOWS_CERTIFICATE` | That base64 text |
| `WINDOWS_CERTIFICATE_PASSWORD` | The password for the `.pfx` |

GitHub limits how large a secret can be. **VERIFY** the current limit (it has been 48 KB). The base64 has to fit. Azure Artifact Signing avoids putting the certificate in a secret, which is the better fit when the `.pfx` is huge or the CA will not export one.

Each sign job imports the `.pfx` into the runner's user certificate store, deletes the `.pfx` file, and signs with the certificate thumbprint. The password is not passed on the `signtool` command line. When that step ends, including when signing fails, the script removes the imported certificate from `Cert:\CurrentUser\My` by thumbprint and deletes the CNG key container that held the private key. The last step of each sign job does that removal again if the thumbprint file or the key-container file is still there, including when an earlier step failed.

An ephemeral key was the other way to avoid leaving the private key on the runner. `signtool` is a separate process, and it can only see a key the certificate store still has, so the import keeps the key for the length of the sign step and the cleanup deletes the container afterwards.

Use GitHub-hosted runners only. This workflow already selects `windows-latest` and `ubuntu-latest`. A self-hosted runner keeps the user profile between jobs, so a certificate that survived the cleanup would remain on that machine. Do not point this workflow at a self-hosted runner.

The timestamp server defaults to `http://timestamp.digicert.com`. **VERIFY** the timestamp URL your CA documents, and set the repository variable `WINDOWS_TIMESTAMP_URL` if theirs is different. It has to start with `http://` or `https://`.

`scripts/sign-windows.ps1` calls `signtool sign /fd SHA256 /sha1 <thumbprint> /tr <timestamp> /td SHA256` on each file. On GitHub Actions, `signtool.exe` is taken from the Windows SDK under `Windows Kits\10`. `TAURI_WINDOWS_SIGNTOOL_PATH` and `PATH` are ignored there, so a build step cannot point the sign job at a stand-in. The workflow does not set a Tauri `signCommand`, so the compiler never sees the certificate.

Set `SIGNING_ENABLED` to `true` last.

### What the switch changes

`SIGNING_ENABLED=true` does not by itself select the signed path. The compile job also requires a push of a `v*` tag. A **Run workflow** dispatch stays on the unsigned job and skips signing.

On a `v*` tag with the switch on, the unsigned job is skipped. Four jobs run instead:

1. **Compile Windows binaries** has no `release` environment and no certificate. It runs `npm run tauri build -- --no-bundle`, copies `cfw-zero-touch.exe` and `cfw-flash.exe` (and any other executable beside them in `src-tauri/target/release`), and publishes the SHA-256 of those files as a job output.
2. **Sign Windows binaries** uses the `release` environment, so GitHub waits for the reviewer. It checks out `scripts/` only, with credentials persistence off. It does not run `npm` or `cargo`. It downloads the unsigned executables, refuses to continue if their hashes differ from the compile job's output, and only then signs them. It publishes the hashes of the signed executables.
3. **Bundle Windows installers** has no `release` environment and no certificate. It puts the signed executables back into `src-tauri/target/release`, checks those hashes, and runs `npm run tauri bundle -- --no-binary-patching --no-sign`. `--no-binary-patching` needs the Tauri CLI at 2.12.2. Tauri otherwise rewrites `cfw-zero-touch.exe` once per installer type (MSI, then NSIS), which strips an Authenticode signature. This app does not use the updater, so that per-installer patch is not required. The bundle job hashes the executables again afterwards and stops if the bytes changed. It publishes the hashes of the unsigned setup `.exe` and `.msi`.
4. **Sign Windows installers** uses the `release` environment again, so GitHub asks for approval a second time. It checks out `scripts/` only, downloads the installers, checks their hashes, and signs the setup `.exe` and the `.msi`. `SHA256SUMS` is written after that, so the hashes match the signed installers.

Approve both waits. The bundle job between them never receives the certificate.

`SIGNING_METHOD=azure` runs the Azure sign step in each sign job. `pfx`, or an empty method, runs the `.pfx` sign step. A different spelling fails the guard on a tag push before the compile.

After the installers are signed, the job runs `signtool verify /pa` on the setup `.exe` and the `.msi`. It then extracts the MSI with `msiexec /a` (the copy in `System32`, so `PATH` cannot substitute another `msiexec`) and runs `signtool verify /pa` on every `.exe` inside. `cfw-zero-touch.exe` and `cfw-flash.exe` have to be in that extract. A failure there fails the job, so a broken signature is not uploaded as a signed build. The unsigned path runs the same extract and requires those checks to fail.

## Check a file you downloaded

Do this for a draft and again for anything you publish. Check both the setup `.exe` and the `.msi`. `SIGNING.txt` should say `unsigned` or `signed`, matching the banner.

### SHA256SUMS

`SHA256SUMS` uses lowercase hex, two spaces, then the file name, one file per line. The file itself is not listed.

On Windows:

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath '.\the-setup.exe'
```

Compare the hash to the line for that file name. PowerShell prints uppercase letters. The file uses lowercase. Ignore the case of the letters.

With GNU `sha256sum`, from the folder that contains the files:

```bash
sha256sum -c SHA256SUMS
```

`OK` on each line means the bytes match the file that was uploaded.

### signtool verify /pa

`signtool.exe` comes with the Windows SDK. A Developer Command Prompt already has it on `PATH`. A typical path is:

`C:\Program Files (x86)\Windows Kits\10\bin\<version>\x64\signtool.exe`

```bat
signtool verify /pa "the-setup.exe"
signtool verify /pa "the-setup.msi"
```

`/pa` selects the default Authenticode verification policy. That is the same trust check Windows uses to decide whether a signature chains to a trusted root.

An unsigned file fails this command. That failure is what you want while signing is off, and the release job records it. A signed file should report that it was successfully verified. Check the setup `.exe` and the `.msi`, because each one has its own signature.

Also extract the MSI and check the programs it installs. The app file is `cfw-zero-touch.exe`. The window title is CFW Card Studio. The admin helper beside it is `cfw-flash.exe`.

```bat
msiexec /a "the-setup.msi" /qn TARGETDIR=C:\cfw-msi-extract\
signtool verify /pa "C:\cfw-msi-extract\path\to\cfw-zero-touch.exe"
signtool verify /pa "C:\cfw-msi-extract\path\to\cfw-flash.exe"
```

Search the extract and run `signtool verify /pa` on every `.exe`. An unsigned build fails each of them. A signed build passes each of them. Signing only the outer setup file leaves these two unsigned, which is why the tag pipeline signs them before it bundles.

## What you do yourself

- Publish the draft, from the releases page, after the checks above.
- Choose a signing vendor and pay them, later, after you **VERIFY** the price.
- Approve the `release` environment twice when a signed tag build is waiting: once for the executables, and once for the installers.

The workflow builds, checksums, and opens the draft. With the switch off, it labels the build unsigned and skips signing.
