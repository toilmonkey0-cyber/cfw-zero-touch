import { describe, expect, it } from "vitest";
import { formatBytes } from "./formatBytes";

describe("formatBytes", () => {
  it("labels an unknown size and whole bytes under 1000", () => {
    expect(formatBytes(-1)).toBe("unknown size");
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
  });

  it("uses one decimal under 10 MB and rounds from there up to 1 GB", () => {
    expect(formatBytes(1_500_000)).toBe("1.5 MB");
    expect(formatBytes(12_600_000)).toBe("13 MB");
    expect(formatBytes(999_000_000)).toBe("999 MB");
  });

  it("switches to gigabytes at 1000 MB", () => {
    expect(formatBytes(1_000_000_000)).toBe("1.0 GB");
    expect(formatBytes(2_500_000_000)).toBe("2.5 GB");
  });
});
