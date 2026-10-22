# Tester checklist

Thank you for testing CFW Card Studio. This is the whole job. Go from top to bottom, and send a report even if you stop early.

The app writes SD cards for handhelds such as the R36S. These steps check the real card on your desk. Take your time on the safety section. A careful backup is the most useful thing you can do.

## What you need

- Your handheld, and the model printed on it (for example R36S).
- The board name, if you know it. That is the text printed on the circuit board, such as `G80CA-MB V1.3`. Write `unknown` if you have not already seen it. You do not need to open the handheld for this beta.
- The SD card that came with the handheld: the brand, and the size printed on the card (for example Samsung 64 GB).
- A spare SD card you can erase. It needs to be the same printed size as the stock card, or larger. Write down its brand and printed size too.
- A card you already know is fake, for the fake-card test only. A fake card is sold as a large size and really holds less. Use one you already own.
- A Windows 10 or 11 PC, a card reader, and the Card Studio build from this beta.
- The free key. Aaron will send you a key. On the start screen, choose **View license**, then **Enter key**, and paste it. Backup, restore, the card tests, guided setup, and **Save support file** also work before the key is entered.

Write these down before you start. The report asks for them.

- Device model
- Board name, or `unknown`
- Stock card brand and printed size
- Spare card brand and printed size
- Windows version, from Settings → System → About
- App version. On the start screen, under the title, the line begins with `Card Studio`

## Safety first

1. Back up the stock card with the app before any other test.
2. On every disk list, read the disk number and the size. They need to match the card in the reader. The size on screen is often a little smaller than the number printed on the card. A card marked 64 GB often shows about 59 GB. That gap is normal. A size of 0 is a reason to stop.
3. Stop and send a bug report if the computer's main drive appears in the list, or if disk 0 appears.
4. Type a confirmation word only when the disk number and the size match the card you mean to use. The words are `PROBE`, `FULL`, and `FLASH`. Each step asks for its own word.
5. After the backup, take the stock card out and set it aside. Tests that write the card use the spare card, or a fake card you already plan to erase.
6. Keep the backup on the computer. You need the file ending in `.img.gz` and the `.sha256` file beside it.
7. Leave game files out of the report. Leave the backup file out of the report too. The backup can contain games that were already on the card. The report needs the checksum text and the support zip.

If Windows refuses the backup read, close the app, run Card Studio as administrator, and try that backup again.

## How to mark a test

- **Pass:** every check in that test happened.
- **Fail:** a check did not happen, or the app offered the wrong disk. Copy the exact words on screen.
- **Did not run:** you skipped it. Say why, in a few words.

## 1. Back up the stock card, restore it, then boot

This checks that a backup can be written onto a spare card and that the handheld still starts.

### Back up the stock card

1. Put only the stock card in the reader.
2. On the start screen, choose **Back up a stock card**.
3. Choose **Choose where to save**, and pick a folder on the computer. The file name needs to end in `.img.gz`.
4. Find the row whose size matches the stock card. Choose **Back up disk** and that disk's number.
5. Wait until the app says the file is saved and shows a SHA-256 checksum. Leave Card Studio open if you can. A finished backup is what lets a later card test run on that same card until you close the app.
6. Confirm the folder contains the `.img.gz` file and a `.sha256` file next to it.
7. Eject the stock card and set it aside.

**Pass:** the app says the backup was saved, both files are on the computer, and the list showed the stock card.

**Fail:** an error, a missing file, a size that does not match the card in your hand, or the computer's main drive in the list.

**Report:** the disk number, the size on screen, the printed size, and the SHA-256 the app showed. Keep the `.img.gz` file on your computer.

### Restore onto the spare card

1. Put only the spare card in the reader. It needs to be the same size as the stock card, or larger.
2. On the start screen, choose **Restore a backup**.
3. Choose **Choose backup**, and pick the finished `.img.gz` file. Leave the `.sha256` file in the same folder.
4. Read the disk number, the name, and the size. They need to be the spare card.
5. Type `FLASH` in that row. Choose **Restore onto disk** and that number.
6. Windows may ask for approval. Approve it for the spare card.
7. Wait until the app says the card matches the backup.

**Pass:** the app says the restore matched, and the spare card is the disk it wrote.

**Fail:** the app offers a different disk, the checksum does not match, or it stops with an error. Copy the words on screen. If the wrong disk was written, stop here and send a bug report with severity **critical**.

### Boot the handheld

1. Eject the spare card in Windows.
2. Put the spare card in the handheld.
3. Turn the handheld on, and wait for the game menu.
4. Shut the handheld down from the menu.

**Pass:** the handheld reaches the game menu.

**Fail:** it stays on a black screen, loops, or shows an error. Describe what you saw. Your original stock card is still the card you set aside.

**Report:** whether the game menu appeared, and both card sizes if the spare card is larger than the stock card.

## 2. Fake-card test

Use a card you already know is fake. Keep the stock card and the restored spare card out of the reader.

The quick test writes a short pattern and overwrites part of that card. The full test erases the whole fake card.

The test starts when this visit to the app already has a backup of that exact card, or when the card is blank. If the app will not start the test, say which of those it asked for.

### Quick test

1. On the start screen, choose **Test a card**.
2. Read the disk number and the size. They need to be the fake card.
3. Type `PROBE`.
4. Choose **Test disk** and that number.
5. Approve the Windows prompt.
6. Read the sentence when it finishes.

**Pass:** the sentence says the card is fake, names the size it claims, and names the smaller size it really holds. It also says part of the card was overwritten.

**Fail:** it says the card looks genuine, it never names both sizes, or the computer's main drive was in the list.

**Report:** the full sentence, the size printed on the card, and the size on screen.

### Full test

This erases the whole fake card. Skip it when that card still has files you want. Mark **Did not run** and say why.

1. On the start screen, choose **Full card test**.
2. Confirm the disk number and the size are still the fake card.
3. Type `FULL`.
4. Choose **Erase and test disk** and that number.
5. Read the sentence when it finishes.

**Pass:** the sentence says the card was fully erased. Paste the whole sentence, including any line about bad spots.

**Fail:** it erases a different disk, or it finishes without saying the card was fully erased.

**Report:** the full sentence.

## 3. Cancel a flash, then flash for real

These two passes use **Guided setup** and the spare card. The stock card stays out. The first pass stops on purpose. The second pass writes the spare card, boots the handheld, and checks that the same card is recognized when you put it back.

`OVERWRITTEN` in uppercase means the app is telling you the card was written. A finished flash should show that word. A flash you cancel before Windows approves it should stop without claiming the card was written.

Choose the layout that matches your handheld. Read the button. When it says the layout writes an OS image, do both flash passes below. When it does not, skip **Flash the system** on both passes, mark both flash lines **Did not run**, and write `this layout has no OS image`. Then still boot the spare card and choose **This card is back**.

### First pass: cancel before any write

1. On the start screen, choose **Guided setup**.
2. Put only the spare card in the reader.
3. On **Back up the card**, choose **Skip this step**. The app warns that this setup then cannot restore the card. That is fine for this pass: the stock-card backup from section 1 is already on the computer, and declining the Windows prompt leaves the spare card unwritten.
4. On **Quick card test** and **Full card test**, choose **Skip this step**.
5. On **Check which handheld this is**, choose the layout that matches your handheld. If the app suggests one and you agree, choose **Use this device**.
6. On **Flash the system**, read the disk number and the size. They need to be the spare card. Type `FLASH` and choose **Flash OS**.
7. When Windows asks for approval, choose No.
8. Copy the exact words on screen.

The setup stops after this. That stop is expected. Choose **Start over** before the second pass.

**Pass:** the app says the flash did not finish, or that it does not know whether the card was written. It should not say the card was overwritten, and it should not show `OVERWRITTEN` in uppercase.

**Fail:** the app says the card was overwritten after you declined the Windows prompt.

### Second pass: flash, boot, and put the same card back

1. Choose **Start over**, or go back to the start screen and choose **Guided setup** again.
2. Put only the spare card in the reader.
3. On **Back up the card**, save a backup of the spare card and wait until it finishes. This reads the whole spare card.
4. On **Quick card test** and **Full card test**, choose **Skip this step**.
5. On **Check which handheld this is**, choose the same layout as before.
6. On **Flash the system**, read the disk number and the size again. Type `FLASH` only for the spare card. Choose **Flash OS**, and approve the Windows prompt.
7. When the flash finishes, copy the words on screen before you continue.
8. Follow **Boot the handheld once**. Eject the card, put it in the handheld, wait for the game menu, shut down, and put the same card back in the computer.
9. Choose **This card is back**.

**Pass for the flash:** the finished-flash sentence contains `OVERWRITTEN`, because the spare card was written.

**Fail for the flash:** a finished flash never shows `OVERWRITTEN`, or the word appears for a disk that is not the spare card.

**Pass for the card coming back:** the app says the card is back, and that the disk number, serial, size, and partition table match. The copy step becomes available. You can stop before copying any games.

**Fail for the card coming back:** the app says this same card was not recognized, or it says `This card did not report a serial, so it cannot be recognized.` If that happens, mark the different-card check **Did not run**. The setup has already stopped.

**Report:** the exact sentence after the cancel, the exact sentence after the finished flash, and the sentence after **This card is back**.

### A different card

Do this at the end of the second pass, while guided setup is still open.

1. Eject the spare card.
2. Put in the stock card, or any other card that is not the spare.
3. Choose **This card is back** or **Check this card again**.

**Pass:** the app stops and says this card was not recognized. Copy stays off. The stock card is unchanged.

**Fail:** the app treats the other card as the spare card, or it starts copying.

**Report:** the sentence on screen, the disk numbers you saw, and whether the reader is a USB reader.

## 4. Stock-card fingerprint

A fingerprint is the file name and the SHA-256 checksum of the small `.dtb` files on the stock card. Those files describe the board. Paste only those files.

The backup only read the stock card. Put the stock card back in the reader. In File Explorer, open the drive named `BOOT`. Leave the games drive alone. That drive is often named `EASYROMS` or `ROMS`.

1. Press the Windows key, type `PowerShell`, and open Windows PowerShell.
2. If `BOOT` is drive `E:`, paste the lines below. Change `E:` when your `BOOT` drive uses another letter.

```powershell
Get-ChildItem -LiteralPath 'E:\' -File -Filter *.dtb |
  Where-Object { $_.Length -le 1MB } |
  Sort-Object Name |
  Select-Object -First 64 |
  Get-FileHash -Algorithm SHA256 |
  Format-Table Hash, Path -AutoSize
```

3. Copy the whole table into the hardware report. Each row is a long hash and a file name, such as `rk3326-evb-lp3-v12-linux.dtb`.

**Pass:** the report includes at least one `.dtb` file name and its hash, from the `BOOT` drive.

**Fail:** `BOOT` never appears, or the only files you can see are on the games drive. Write the drive names you did see.

Opening these files can update the "last opened" time on the card. The games and the system stay as they are.

## 5. Save the support file

**Save support file** is on the start screen, under the title. The same button stays available on the other screens.

1. Choose **Save support file**.
2. Save the zip on the computer. The suggested name is `cfw-card-studio-support.zip`.
3. After you create the hardware report on GitHub, drag that zip onto the issue.

The zip includes the app version, the Windows version, recent notes, the latest card test, the card serial, the partition id, and whether a license is turned on. Game names, your folders, the license key, and email addresses are left out.

**Pass:** the zip saved, and you attached it to the report.

**Fail:** the button did nothing, or the file did not appear. Copy any error on screen.

## Send the report

Send one [hardware report](https://github.com/toilmonkey0-cyber/cfw-zero-touch/issues/new?template=beta-hardware-report.yml) for the cards you tested.

When something breaks, also open a [beta bug](https://github.com/toilmonkey0-cyber/cfw-zero-touch/issues/new?template=beta-bug.yml). Critical means lost data, a card that will not boot and cannot be restored from the backup you made, or the wrong disk offered or written. Stop testing and send that bug report first.

More about this beta is in the [beta notes](README.md).

## Trusted tester

This part is optional. It is for people who are comfortable with a spare card and a short script.

- Hash the stock card from the backup file. [Stock-card fingerprints](../FINGERPRINTS.md) and `scripts/hash-dtb.sh` show the read-only steps. Mount the `BOOT` partition of a copy of the backup, run the script, and paste the same file names and hashes.
- If your handheld's layout has no OS image, you can still check the flash message on the spare card. Choose a layout whose button says it writes an OS image, run the two flash passes in section 3, and copy the words on screen. The handheld may never show a game menu for that image. When you are finished with the spare card, put the original stock card back in the handheld.
- Add the panel name only when it is printed on the screen or you already know it from this card. Leave it blank when you would be guessing.
- From Windows Disk Management, you can add the stock card's partition count, type, label, and size. Skip this when that screen is unfamiliar.
- On the fake card only, start **Test a card** and choose **Cancel test** once. Report whether the app says data was overwritten, or says nothing was overwritten.
- With the spare card and a second card in another reader, confirm that copy stays off until the app recognizes the spare card again.
- When a flash or a test stops halfway, copy the exact sentence, especially any line that contains `OVERWRITTEN`.
