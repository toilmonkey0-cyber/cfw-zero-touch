import { describe, expect, it } from "vitest";
import { probeConfirmPrompt } from "./probePath";

describe("probeConfirmPrompt", () => {
  it("asks for PROBE and names the disk, model, and size", () => {
    const prompt = probeConfirmPrompt({
      number: 2,
      name: "Generic SD",
      sizeLabel: "64.0 GB",
    });
    expect(prompt).toContain("PROBE");
    expect(prompt).toContain("disk 2");
    expect(prompt).toContain("Generic SD");
    expect(prompt).toContain("64.0 GB");
    expect(prompt).not.toContain("FLASH");
  });
});
