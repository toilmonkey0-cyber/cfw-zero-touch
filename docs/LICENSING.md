# Licensing

Card Studio is MIT-licensed source. Signed builds, update support, and human support are a separate $12 one-time purchase, checked with a Gumroad license key. The check is optional. Backup, restore, the quick card test, the full card test, guided setup, and the disk refusals all run with no license.

This is not DRM. The verification code ships in the open repository. A person who can change the program can skip the check. The cache and the machine binding are a speed bump against casual copying of a license file, not a control that holds against a determined reader of the source.

## What Gumroad is asked

Activation and re-checks are `POST https://api.gumroad.com/v2/licenses/verify` with three form fields: `product_id`, `license_key`, and `increment_uses_count` (`true` or `false`). There is no API token in the app. Gumroad's public verify response is read for `success`, `uses`, and `purchase.refunded`, `purchase.chargebacked`, `purchase.disputed`, `purchase.dispute_won`, `purchase.test`, and `purchase.product_id`. Other purchase fields, including the echoed license key and the buyer email, are ignored and are not stored.

`increment_uses_count` is `false` on the first look and on every later re-check. It is `true` only for the second call of a new activation on this computer, after the first call has already shown a usable purchase. A refunded, chargebacked, still-open dispute, release-build test purchase, unrecognized, or mismatched response never increments. Launching the app does not increment.

The product id is compiled in with `CFW_GUMROAD_PRODUCT_ID`. When that is unset, the binary contains the placeholder `REPLACE_WITH_GUMROAD_PRODUCT_ID` and activation does not call Gumroad. Do not put a seller token in the build.

## What is rejected

A key is refused, in plain language and without repeating the key, when:

- Gumroad reports `success: false` (the key is not recognized).
- `purchase.refunded` is true.
- `purchase.chargebacked` is true.
- `purchase.disputed` is true and `purchase.dispute_won` is false. The sale is still in dispute. When both are true, the seller kept the sale and the key is accepted. `disputed: false` with `dispute_won: true` is accepted too. If `disputed` is true and `dispute_won` is missing, the response is unreadable.
- `purchase.test` is true and this is a release build. Debug builds (`debug_assertions`, including a local `tauri dev` build) accept a test purchase so a Gumroad test key can be tried. The installer is a release build and leaves the key inactive. A missing `test` flag is unreadable, not treated as a real purchase.
- `purchase.product_id` is not the id this build asked about.
- The key is not 8 to 128 letters, digits, and hyphens.
- A required purchase flag is missing. A missing flag is not treated as false.

`purchase.refunded` is the refund signal from the verify response. A partial-refund field is not part of that check.

## Offline cache

One successful activation writes `%LOCALAPPDATA%\cfw-card-studio\license\activation.json` (or `$CFW_STUDIO_DATA/license/activation.json` when that override is set). The file holds the product id, the license key (so a later re-check does not ask again), the SHA-256 of this computer's machine id, the Gumroad `uses` count, and the time of the last successful check. It does not hold the buyer email or the rest of the purchase.

The machine material is `/etc/machine-id` on Linux and the Windows `MachineGuid`. The raw value is hashed before it is written. The file also carries an HMAC-SHA256 over the payload. The HMAC key is derived from a fixed label and that same machine hash. Both are knowable from the source and from the computer, so anyone who wants to forge a cache for their own machine can. Editing the file, or copying it to another computer, fails the check and the app asks for the key again. The saved key is not sent anywhere when that check fails.

On Unix the cache directory is mode `0700` and the file is mode `0600`. On Windows the file lives in the per-user LocalAppData profile. This unit does not add a custom DACL; the profile directory is already private to the user.

For 7 days after a successful check the app does not contact Gumroad. After that, the next launch tries one re-check with `increment_uses_count=false`. A HTTP 200 or a Gumroad `success: false` / refund / chargeback / open dispute / release-build test purchase updates or removes the cache. A timeout, DNS failure, TLS failure, HTTP 5xx, redirect, or a body this app cannot read leaves the cache in place. The same is true after 90 days. The 90 days is only when the wording changes to say the last check was a long time ago. A network failure never turns a saved license off.

`license_status` and `activate_license` are async commands. The verify call runs on a blocking worker (`spawn_blocking`), so an offline re-check can take the full timeout without freezing the window. Launch does not wait on the network before the rest of the UI can run. A cache younger than 7 days does not open a connection.

A timestamp in the future is treated as due for a re-check. Turning the clock back, or recomputing the HMAC, is the same class of bypass as editing the program.

Deleting the cache, or reinstalling without it, counts as a new activation the next time the key is entered, because Gumroad's counter is the only seat count the app can see.

## Three computers

Gumroad's `uses` count is the seat counter. Three is included. The response after the incrementing call is what this computer stores.

Above 3, the app still activates and signed-build support stays available. The status line says the license is already in use on more than 3 computers and that Card Studio keeps working on this one. A hard lock was rejected: the program is open source, so a lock would only stop people who do not read it, and a reinstall increments the counter, so a lock would also punish a buyer who replaced a disk.

## What a license does not gate

These stay available with no key, an invalid key, a refunded key, or a cached key:

- ROM-card prep (prepare, seed, plan, and copy)
- Stock-card backup and its cancel
- Restore and its cancel
- Quick card test (probe), including a fake-card result, and its cancel
- Full card test and its cancel
- Disk pinning, identity checks, and OVERWRITTEN reporting
- Guided setup, which calls those commands
- Format and flash decisions, including the refusals for a fixed disk, a size of 0, disk 0, and a mismatched checksum
- The profile feed and the public release link, which carry card-layout fixes

The license screen is the commercial entitlement for signed builds and support. It does not download a private update package and it does not create a Gumroad product.

## Free and paid

Free, with no key, and without waiting on a license check:

- ROM-card prep: list the card, prepare folders, seed the layout, plan, and copy
- Stock-card backup and restore, including cancel
- Quick card test (probe), including a fake-card result, and the full card test
- Disk pinning and identity checks, including the disk 0 refusal
- OVERWRITTEN reporting
- Guided setup, format and flash decisions, and the disk refusals
- The profile feed and the public release link

Paid. Choosing one with no key opens the Enter key dialog:

- Signed builds
- Support

A valid key unlocks those two on this computer. A refunded, charged-back, or unrecognized key leaves them locked and shows the reason in plain language. More than 3 computers still unlocks, with a warning. After one activation, a failed re-check (offline) stays unlocked. Deactivate this computer from View license; that deletes the saved activation and does not call Gumroad. This doesn't free up one of the 3 computers on Gumroad. Card tools keep working.

`paid_feature` is the command entry for signed builds and support. A safety name returns before any license session is opened, so the check cannot slow those paths. Disk writes are not hooked.

## Network posture

The verify URL is fixed to `https://api.gumroad.com/v2/licenses/verify`. Any other URL is refused before a request is built. The HTTP client is ureq with the existing `tls` feature (rustls and the webpki root set), certificate validation left on, `https_only`, zero redirects, no proxy from the environment, a 5 second connect timeout, and a 15 second overall timeout. ureq can still let a slow DNS lookup run past that overall timeout; that limitation is in the HTTP library. Redirects are not followed, including a 307 that would otherwise replay a POST. That wait happens on the blocking pool, not on the UI thread.

License keys are not written to the diagnostics log. Log lines are reason codes (`refunded`, `unreachable`, `active`, and so on). Debug formatting of a verify call prints `****` instead of the key and does not print response bodies. Gumroad's own error text is not shown, because it can echo the key.
