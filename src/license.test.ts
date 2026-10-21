import { describe, expect, it } from "vitest";
import {
  commercialUnlocked,
  SAFETY_ACTIONS,
  SAFETY_NOTE,
  safetyActions,
  safetyNote,
  statusText,
  type LicenseView,
} from "./license";

const inactive: LicenseView = {
  state: "inactive",
  reason: "not_activated",
  message: "Enter the license key from your Gumroad receipt to activate this computer.",
  uses: null,
  overSoftLimit: false,
  stale: false,
  signedUpdates: false,
  support: false,
  safetyOpen: true,
};

const overLimit: LicenseView = {
  state: "active",
  reason: "active",
  message:
    "This license is already in use on more than 3 computers. Card Studio keeps working on this one.",
  uses: 4,
  overSoftLimit: true,
  stale: false,
  signedUpdates: true,
  support: true,
  safetyOpen: true,
};

describe("license panel", () => {
  it("keeps the same safety actions with no license and when over the soft limit", () => {
    expect(safetyActions()).toEqual(SAFETY_ACTIONS);
    expect(safetyNote()).toBe(SAFETY_NOTE);
    expect(safetyNote()).toBe(safetyNote());
    expect(SAFETY_ACTIONS).toEqual(["backup", "restore", "probe", "fullcheck"]);
  });

  it("does not echo a typed license key back in the status line", () => {
    const key = "ABCD-EFGH-IJKL-MN99";
    expect(statusText(`rejected ${key}`, key)).toBe(
      "Enter the license key from your Gumroad receipt to activate this computer.",
    );
    expect(statusText(inactive.message, key)).toBe(inactive.message);
    expect(statusText(overLimit.message, `  ${key}  `)).not.toContain(key);
  });

  it("unlocks signed updates and support only when the backend says so", () => {
    expect(commercialUnlocked(null)).toEqual({ updates: false, support: false });
    expect(commercialUnlocked(inactive)).toEqual({ updates: false, support: false });
    expect(commercialUnlocked(overLimit)).toEqual({ updates: true, support: true });
  });
});
