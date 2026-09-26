# ArkOS / dArkOS first boot

Source read for this recipe: [dArkOSRE-R36 `expandtoexfat.sh`](https://github.com/southoz/dArkOSRE-R36/blob/main/files/BOOT/expandtoexfat.sh). The official ArkOS wiki says not to expand `EASYROMS` by hand before the first boot, or the boot can hang.

That script is the wipe:

1. If `/boot/doneit` is missing, it grows partition 3, writes `doneit`, and reboots.
2. On the next run, `doneit` exists, so it does not stop. It runs `mkfs.exfat -L EASYROMS` on the games partition and extracts `/roms.tar`.
3. After that it deletes `/boot/firstboot.sh`, deletes itself, and disables `firstboot.service`.

`doneit` is not a disarm. Copying games onto an unmodified card before this script is gone loses those games.

The PC disarm implemented here deletes `expandtoexfat.sh` and `firstboot.sh` from the boot folder and does not create `doneit`. It does not resize partitions, format exFAT, or extract `roms.tar`. Those steps are still required before this disarm is a full first-boot replacement. Until they exist, do not copy a library onto a freshly flashed ArkOS card.
