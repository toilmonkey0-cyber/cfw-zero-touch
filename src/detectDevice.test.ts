import { describe, expect, it } from "vitest";
import {
  MANUAL_DEVICE_MESSAGE,
  detectionNotice,
  profileAfterConfirm,
  type StockDetection,
} from "./detectDevice";

const matched: StockDetection = {
  status: "matched",
  profileId: "r36s-clone-card",
  device: "R36S clone",
  screen: "V12 panel",
  profileName: "R36S clone — games card (internal firmware)",
  message: "",
};

describe("stock device detection", () => {
  it("uses the manual picker message when the card is unknown", () => {
    const notice = detectionNotice(
      {
        status: "manual",
        profileId: "",
        device: "",
        screen: "",
        profileName: "",
        message: MANUAL_DEVICE_MESSAGE,
      },
      false,
    );
    expect(MANUAL_DEVICE_MESSAGE).toBe(
      "We couldn't tell which handheld this is. Please pick it from the list.",
    );
    expect(notice.message).toBe(MANUAL_DEVICE_MESSAGE);
    expect(notice.showConfirm).toBe(false);
    expect(notice.suggestedProfileId).toBe("");
    expect(profileAfterConfirm(matched, false)).toBeNull();
  });

  it("pre-selects a matched device and waits for confirm", () => {
    const notice = detectionNotice(matched, false);
    expect(notice.showConfirm).toBe(true);
    expect(notice.suggestedProfileId).toBe("r36s-clone-card");
    expect(notice.message).toContain("R36S clone");
    expect(notice.message).toContain("V12 panel");
    expect(notice.message).toContain("R36S clone — games card (internal firmware)");
    expect(profileAfterConfirm(matched, false)).toBeNull();
    expect(profileAfterConfirm(matched, true)).toBe("r36s-clone-card");
  });

  it("clears the suggestion when the user chooses a different layout", () => {
    const notice = detectionNotice(matched, true);
    expect(notice.message).toBe("");
    expect(notice.showConfirm).toBe(false);
    expect(notice.suggestedProfileId).toBe("");
    expect(profileAfterConfirm(null, true)).toBeNull();
    expect(
      profileAfterConfirm(
        { ...matched, status: "manual" },
        true,
      ),
    ).toBeNull();
  });
});
