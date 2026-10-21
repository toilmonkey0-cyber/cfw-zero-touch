import { describe, expect, it } from "vitest";
import { runFreeRomCardPrep, type Invoke } from "./romCardPrep";

const ready = {
  id: "E:",
  decision: "ready" as const,
  totalBytes: 32_000_000_000,
  reason: "",
};

function mockInvoke(hangLicense: boolean): { invoke: Invoke; calls: string[] } {
  const calls: string[] = [];
  const invoke: Invoke = async <T>(command: string) => {
    calls.push(command);
    if (
      command === "license_status" ||
      command === "activate_license" ||
      command === "deactivate_license" ||
      command === "paid_feature"
    ) {
      if (hangLicense) {
        await new Promise(() => {});
      }
      throw new Error(`ROM prep consulted ${command}`);
    }
    switch (command) {
      case "list_volumes":
        return [ready] as T;
      case "prepare_card":
        return { ...ready, decision: "ready" } as T;
      case "seed_card":
        return { folders: ["gba", "nes"] } as T;
      case "plan_roms":
        return { copyCount: 2, skipCount: 0, files: [], warning: null, duplicates: [], duplicateSkipped: 0 } as T;
      case "copy_roms":
        return { copied: 2, skipped: 0, bytesCopied: 4096 } as T;
      default:
        throw new Error(`unexpected command ${command}`);
    }
  };
  return { invoke, calls };
}

describe("free ROM-card prep", () => {
  it("prepares, seeds, plans, and copies with no license key", async () => {
    const { invoke, calls } = mockInvoke(true);
    const result = await Promise.race([
      runFreeRomCardPrep(invoke, {
        profileId: "arkos-roms-card",
        volumeId: "E:",
        confirmation: "",
        displayedBytes: ready.totalBytes,
        library: "C:\\roms",
        include: ["gba", "nes"],
      }),
      new Promise<never>((_resolve, reject) => {
        setTimeout(() => reject(new Error("ROM prep waited on licensing")), 400);
      }),
    ]);
    expect(calls).toEqual(["list_volumes", "prepare_card", "seed_card", "plan_roms", "copy_roms"]);
    expect(result.folders).toEqual(["gba", "nes"]);
    expect(result.plan.copyCount).toBe(2);
    expect(result.report.copied).toBe(2);
    expect(result.report.bytesCopied).toBe(4096);
  });

  it("sends the typed FORMAT confirmation only when the volume needs a format", async () => {
    const seen: Record<string, unknown>[] = [];
    const invoke: Invoke = async <T>(command: string, args?: Record<string, unknown>) => {
      seen.push({ command, ...(args ?? {}) });
      if (command === "list_volumes") {
        return [{ ...ready, decision: "needs_format" }] as T;
      }
      if (command === "prepare_card") return { ...ready } as T;
      if (command === "seed_card") return { folders: ["gba"] } as T;
      if (command === "plan_roms") {
        return { copyCount: 1, skipCount: 0, files: [], warning: null, duplicates: [], duplicateSkipped: 0 } as T;
      }
      if (command === "copy_roms") return { copied: 1, skipped: 0, bytesCopied: 10 } as T;
      throw new Error(command);
    };
    await runFreeRomCardPrep(invoke, {
      profileId: "rocknix-roms-card",
      volumeId: "E:",
      confirmation: "FORMAT",
      displayedBytes: ready.totalBytes,
      library: "/home/roms",
      include: ["gba"],
    });
    const prepare = seen.find((row) => row.command === "prepare_card");
    expect(prepare?.confirmation).toBe("FORMAT");
    expect(prepare?.displayedBytes).toBe(ready.totalBytes);
  });
});
