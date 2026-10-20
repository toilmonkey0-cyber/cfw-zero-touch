import { describe, expect, it } from "vitest";
import { stockImagePath } from "./backupPath";

describe("stockImagePath", () => {
  it("keeps a finished gzip image name", () => {
    expect(stockImagePath("stock-card.img.gz")).toBe("stock-card.img.gz");
    expect(stockImagePath("  D:\\Backups\\r36s.img.gz  ")).toBe("D:\\Backups\\r36s.img.gz");
  });

  it("adds the compressed image suffix when the dialog omits it", () => {
    expect(stockImagePath("stock-card")).toBe("stock-card.img.gz");
    expect(stockImagePath("stock-card.gz")).toBe("stock-card.img.gz");
    expect(stockImagePath("stock-card.img")).toBe("stock-card.img.img.gz");
  });
});