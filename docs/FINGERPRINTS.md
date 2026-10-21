# Stock-card fingerprints

A shipped fingerprint is the file name and sha256 of a `.dtb` from a stock card, or from a U3 backup of one. The profile feed (`profiles/*.json` and `profiles-feed.json`) is the only place those hashes live. Placeholder hashes stay in `src-tauri/tests/fixtures` and in test-only profiles.

The screen is confirmed by the user. Leave `screen` unset unless you recorded a panel name from that same card. Leave `partitions` unset unless you recorded that card's real partition table (count, type, label, and size). With no recorded layout, matching uses the DTB name and the hash.

A profile with no real fingerprint is `manualOnly` and has no variants.

The one shipped example is `rk3326-evb-lp3-v12-linux.dtb` for board G80CA-MB V1.3, cited from [DTB Vault](https://dtbvault.com/en/file/rk3326-evb-lp3-v12-linux-IYZchb-P):

```json
"stockFingerprint": {
  "manualOnly": false,
  "source": "https://dtbvault.com/en/file/rk3326-evb-lp3-v12-linux-IYZchb-P",
  "variants": [
    {
      "device": "R36S clone (board G80CA-MB V1.3)",
      "dtbs": [
        {
          "name": "rk3326-evb-lp3-v12-linux.dtb",
          "sha256": "8c4b6b738bfd5af9638db9c6e9a3ac49884ca4b773c4053a31d1a934b69246b0"
        }
      ]
    }
  ]
}
```

`source` is an `http` or `https` URL, or the phrase `dumped from stock card` plus a short description of the card.

## Which files to hash

Hash the top-level `*.dtb` files on the boot volume (the one labeled `BOOT`). Skip a file larger than 1 MiB. Hash at most 64 files. The games partition (`EASYROMS`) is not part of the fingerprint.

Reading those files through a mounted FAT partition may update access times. That update is metadata only.

## From a mounted stock card

The boot volume is already mounted by the operating system. From the repo root:

```bash
scripts/hash-dtb.sh /path/to/BOOT
```

One file:

```bash
scripts/hash-dtb.sh /path/to/BOOT/rk3326-evb-lp3-v12-linux.dtb
```

Windows PowerShell, with `E:` as the boot volume:

```powershell
Get-ChildItem -LiteralPath 'E:\' -File -Filter *.dtb |
  Where-Object { $_.Length -le 1MB } |
  Sort-Object Name |
  Select-Object -First 64 |
  Get-FileHash -Algorithm SHA256
```

## From a U3 backup

A U3 backup is the app's whole-card `name.img.gz`. Decompress a copy to a regular file you own, mount the BOOT partition of that image read-only, and run the same command against the mount:

```bash
gzip -dc /path/to/stock.img.gz > /tmp/stock.img
scripts/hash-dtb.sh /mnt/boot
```

## Add it to the feed

1. Put the file name and hash on the profile in `profiles/<id>.json`, with `manualOnly` set to `false` and a real `source`.
2. Regenerate the checked-in feed:

```bash
node -e "const fs=require('fs'),path=require('path');const dir='profiles';const profiles=fs.readdirSync(dir).filter(f=>f.endsWith('.json')).sort().map(f=>JSON.parse(fs.readFileSync(path.join(dir,f),'utf8')));const feed={feedVersion:1,app:{version:'0.1.0',releaseUrl:'https://github.com/toilmonkey0-cyber/cfw-zero-touch/releases'},profiles:profiles.map(p=>({...p,version:p.version??1}))};fs.writeFileSync('profiles-feed.json',JSON.stringify(feed,null,2)+'\n');"
```

The anti-drift check fails when a shipped hash equals the sha256 of any file under `src-tauri/tests/fixtures`.
