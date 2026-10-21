import { describe, expect, it } from "vitest";
import { runSafetyPaths, type SafetyInvoke } from "./safetyPaths";

describe("safety paths with no license", () => {
  it("runs identity, probe, overwritten reporting, and backup without waiting on a license", async () => {
    const calls: string[] = [];
    const invoke: SafetyInvoke = async <T>(command: string, args?: Record<string, unknown>) => {
      calls.push(command);
      if (
        command === "license_status" ||
        command === "activate_license" ||
        command === "deactivate_license" ||
        command === "paid_feature"
      ) {
        await new Promise(() => {});
      }
      if (command === "read_disk_identity") {
        if (args?.diskNumber === 0) {
          throw new Error("refusing to open disk 0");
        }
        return {
          diskNumber: 2,
          serial: "fixture-card",
          sizeBytes: 8192,
          partitionTableId: "MBR-AABBCCDD",
        } as T;
      }
      if (command === "probe_card") {
        return {
          status: "finished",
          kind: "fake",
          overwritten: true,
          summary:
            "This card is fake. OVERWRITTEN=yes. Part of this card was overwritten. Format the card again before you use this card.",
        } as T;
      }
      if (command === "backup_stock_card") {
        return {
          status: "complete",
          image: "stock.img.gz",
          sidecar: "stock.img.gz.sha256",
          sha256: "abc",
          bytes: 8192,
        } as T;
      }
      throw new Error(`unexpected ${command}`);
    };

    const report = await Promise.race([
      runSafetyPaths(invoke),
      new Promise<never>((_resolve, reject) => {
        setTimeout(() => reject(new Error("safety path waited on licensing")), 400);
      }),
    ]);

    expect(calls).toEqual([
      "read_disk_identity",
      "read_disk_identity",
      "probe_card",
      "backup_stock_card",
    ]);
    expect(report.diskZero).toContain("disk 0");
    expect(report.identity.serial).toBe("fixture-card");
    expect(report.identity.partitionTableId).toBe("MBR-AABBCCDD");
    expect(report.probe.overwritten).toBe(true);
    expect(report.probe.kind).toBe("fake");
    expect(report.probe.summary).toContain("OVERWRITTEN");
    expect(report.backup.status).toBe("complete");
    expect(report.backup.image).toBe("stock.img.gz");
  });
});
