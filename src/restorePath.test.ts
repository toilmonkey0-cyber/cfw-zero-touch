import { describe, expect, it } from "vitest";
import {
  isFinishedBackupName,
  restoreConfirmPrompt,
  restoreDiskModel,
  restorePhaseLabel,
} from "./restorePath";

describe("restoreConfirmPrompt", () => {
  it("names the disk, its model, and its size before FLASH", () => {
    const prompt = restoreConfirmPrompt({
      number: 2,
      name: "Samsung EVO",
      sizeLabel: "32.0 GB",
    });
    expect(prompt).toContain("FLASH");
    expect(prompt).toContain("disk 2");
    expect(prompt).toContain("Samsung EVO");
    expect(prompt).toContain("32.0 GB");
  });
});

describe("restoreDiskModel", () => {
  it("matches the backend fallback and pipe cleanup", () => {
    expect(restoreDiskModel("Samsung EVO")).toBe("Samsung EVO");
    expect(restoreDiskModel("  Samsung EVO  ")).toBe("Samsung EVO");
    expect(restoreDiskModel("")).toBe("USB disk");
    expect(restoreDiskModel("   ")).toBe("USB disk");
    expect(restoreDiskModel("A|B")).toBe("A/B");
  });
});

describe("isFinishedBackupName", () => {
  it("accepts a finished gzip image and refuses partial names", () => {
    expect(isFinishedBackupName("D:\\Backups\\stock.img.gz")).toBe(true);
    expect(isFinishedBackupName("  /tmp/r36s.img.gz  ")).toBe(true);
    expect(isFinishedBackupName("stock.img.gz.partial")).toBe(false);
    expect(isFinishedBackupName("stock.partial.img.gz")).toBe(false);
    expect(isFinishedBackupName("stock.img")).toBe(false);
  });
});

describe("restorePhaseLabel", () => {
  it("labels verify, write, and read-back", () => {
    expect(restorePhaseLabel("verify")).toBe("Checking the backup");
    expect(restorePhaseLabel("write")).toBe("Writing the card");
    expect(restorePhaseLabel("readback")).toBe("Reading the card back");
  });
});
