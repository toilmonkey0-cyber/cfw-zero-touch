---
name: firstboot-safety
description: Use this when implementing or changing ArkOS/dArkOS single-card prepare, EASYROMS seeding, or firstboot disarm logic.
---
# Firstboot safety

1. Read `docs/MARKET.md` landmine section before coding.
2. Assume firstboot will `mkfs` the ROM partition and extract `roms.tar` unless disarmed.
3. Order of operations on PC: flash → partition/format → seed from roms.tar → disarm firstboot → copy user ROMs.
4. Never tell users to copy ROMs before firstboot on an unmodified image.
5. Add/adjust tests on fixture trees whenever disarm paths change.
