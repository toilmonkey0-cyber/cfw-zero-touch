import { describe, expect, it } from "vitest";
import { fullConfirmPrompt, fullProgressLabel } from "./fullCheckPath";

describe("fullConfirmPrompt", () => {
  it("asks for FULL and names the disk, model, and size", () => {
    const prompt = fullConfirmPrompt({
      number: 2,
      name: "Generic SD",
      sizeLabel: "64.0 GB",
    });
    expect(prompt).toContain("FULL");
    expect(prompt).toContain("disk 2");
    expect(prompt).toContain("Generic SD");
    expect(prompt).toContain("64.0 GB");
    expect(prompt).not.toContain("PROBE");
    expect(prompt).not.toContain("FLASH");
  });
});

describe("fullProgressLabel", () => {
  it("shows percent, speed, and time left", () => {
    expect(
      fullProgressLabel({
        phase: "write",
        percent: 25,
        mbps: 12.5,
        etaSeconds: 90,
      }),
    ).toBe("Writing 25% · 12.5 MB/s · 2 min left");
    expect(
      fullProgressLabel({
        phase: "read",
        percent: 100,
        mbps: 40,
        etaSeconds: 0,
      }),
    ).toBe("Reading 100% · 40.0 MB/s · finishing");
    expect(
      fullProgressLabel({
        phase: "write",
        percent: 10,
        mbps: 1,
        etaSeconds: 12,
      }),
    ).toBe("Writing 10% · 1.0 MB/s · 12s left");
  });
});
