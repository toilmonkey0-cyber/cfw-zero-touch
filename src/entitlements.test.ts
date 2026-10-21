import { describe, expect, it } from "vitest";
import {
  applyStatus,
  deactivateThisComputer,
  FREE_FEATURES,
  initialLicenseUi,
  isFreeFeature,
  PAID_FEATURES,
  paidPanelCopy,
  reduceLicense,
  requestPaidFeature,
  submitLicenseKey,
  type LicenseUiState,
} from "./entitlements";
import type { LicenseView } from "./license";

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

const active: LicenseView = {
  state: "active",
  reason: "active",
  message: "License active on this computer.",
  uses: 1,
  overSoftLimit: false,
  stale: false,
  signedUpdates: true,
  support: true,
  safetyOpen: true,
};

const refunded: LicenseView = {
  state: "inactive",
  reason: "refunded",
  message: "This license was refunded, so it can't be used.",
  uses: null,
  overSoftLimit: false,
  stale: false,
  signedUpdates: false,
  support: false,
  safetyOpen: true,
};

const offline: LicenseView = {
  state: "inactive",
  reason: "unreachable",
  message:
    "Couldn't reach Gumroad to check this license. Try again when you're online. Card tools still work.",
  uses: null,
  overSoftLimit: false,
  stale: false,
  signedUpdates: false,
  support: false,
  safetyOpen: true,
};

const staleActive: LicenseView = {
  state: "active",
  reason: "active",
  message: "Couldn't reach Gumroad to re-check this license. This computer stays activated.",
  uses: 1,
  overSoftLimit: false,
  stale: true,
  signedUpdates: true,
  support: true,
  safetyOpen: true,
};

const tooMany: LicenseView = {
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

const deactivated: LicenseView = {
  ...inactive,
  message:
    "This computer is no longer activated. Enter the license key from your Gumroad receipt to use signed builds and support again. Card tools still work.",
};

function closedPaid(feature: "signed-updates" | "support"): LicenseUiState {
  return reduceLicense(initialLicenseUi(), {
    type: "paid-result",
    feature,
    open: false,
    message: inactive.message,
  });
}

describe("free and paid split", () => {
  it("keeps ROM-card prep and safety tools free", () => {
    for (const name of [
      "rom-card-prep",
      "backup",
      "restore",
      "probe",
      "full-check",
      "guided-setup",
      "flash",
      "identity",
      "overwritten",
      "profile-feed",
    ]) {
      expect(isFreeFeature(name)).toBe(true);
      expect(PAID_FEATURES).not.toContain(name);
    }
    expect(FREE_FEATURES).not.toContain("signed-updates");
    expect(FREE_FEATURES).not.toContain("support");
    expect(PAID_FEATURES).toEqual(["signed-updates", "support"]);
  });
});

describe("unlock dialog", () => {
  it("opens the unlock dialog when a paid feature is requested with no key", async () => {
    const next = await requestPaidFeature(initialLicenseUi(), {
      paid: async () => ({ open: false, message: inactive.message }),
    }, "signed-updates");
    expect(next.dialog).toBe("unlock");
    expect(next.pending).toBe("signed-updates");
    expect(next.unlockedPanel).toBeNull();
    expect(next.notice).toContain("license key");
  });

  it("unlocks the paid feature when the mocked key is valid", async () => {
    const locked = closedPaid("support");
    const next = await submitLicenseKey(
      { ...locked, draftKey: "ABCD-EFGH-IJKL-MN99" },
      { activate: async () => active },
    );
    expect(next.dialog).toBe("closed");
    expect(next.unlockedPanel).toBe("support");
    expect(next.view?.support).toBe(true);
    expect(next.view?.signedUpdates).toBe(true);
    expect(next.draftKey).toBe("");
  });

  it("keeps the paid feature locked when the key was refunded", async () => {
    const locked = { ...closedPaid("support"), draftKey: "REFUNDED-KEY-0001" };
    const next = await submitLicenseKey(locked, { activate: async () => refunded });
    expect(next.dialog).toBe("unlock");
    expect(next.unlockedPanel).toBeNull();
    expect(next.notice).toContain("refunded");
    expect(next.view?.signedUpdates).toBe(false);
    expect(next.view?.support).toBe(false);
  });

  it("stays locked when the first check cannot reach Gumroad", async () => {
    const locked = { ...closedPaid("signed-updates"), draftKey: "ABCD-EFGH-IJKL-MN99" };
    const next = await submitLicenseKey(locked, { activate: async () => offline });
    expect(next.dialog).toBe("unlock");
    expect(next.unlockedPanel).toBeNull();
    expect(next.notice).toContain("Couldn't reach Gumroad");
    expect(next.notice).not.toContain("ABCD-EFGH-IJKL-MN99");
  });

  it("stays unlocked when a saved license cannot reach Gumroad", async () => {
    const saved = applyStatus(initialLicenseUi(), staleActive);
    const next = await requestPaidFeature(saved, {
      paid: async () => ({ open: true, message: staleActive.message }),
    }, "signed-updates");
    expect(next.dialog).toBe("closed");
    expect(next.unlockedPanel).toBe("signed-updates");
    expect(next.view?.stale).toBe(true);
    expect(next.view?.signedUpdates).toBe(true);
    expect(paidPanelCopy("signed-updates", next.view)).toContain("unlocked");
  });

  it("stays unlocked and shows the more-than-3-computers warning", async () => {
    const locked = { ...closedPaid("signed-updates"), draftKey: "ABCD-EFGH-IJKL-MN99" };
    const next = await submitLicenseKey(locked, { activate: async () => tooMany });
    expect(next.dialog).toBe("closed");
    expect(next.unlockedPanel).toBe("signed-updates");
    expect(next.notice).toContain("more than 3");
    expect(paidPanelCopy("signed-updates", next.view)).toContain("more than 3");
  });

  it("does not echo a typed key when activation fails with the key in the error", async () => {
    const key = "ABCD-EFGH-IJKL-MN99";
    const locked = { ...closedPaid("support"), draftKey: key };
    const next = await submitLicenseKey(locked, {
      activate: async () => {
        throw new Error(`rejected ${key}`);
      },
    });
    expect(next.dialog).toBe("unlock");
    expect(next.notice).not.toContain(key);
    expect(next.unlockedPanel).toBeNull();
  });
});

describe("view and deactivate", () => {
  it("opens the license view and deactivates this computer", async () => {
    const viewing = reduceLicense(applyStatus(initialLicenseUi(), active), { type: "manage" });
    expect(viewing.dialog).toBe("manage");
    const next = await deactivateThisComputer(viewing, {
      deactivate: async () => deactivated,
    });
    expect(next.dialog).toBe("manage");
    expect(next.view?.state).toBe("inactive");
    expect(next.unlockedPanel).toBeNull();
    expect(next.notice).toContain("no longer activated");
    expect(next.view?.signedUpdates).toBe(false);
  });

  it("drops an open paid panel when a later status says the license is gone", () => {
    const open = reduceLicense(applyStatus(initialLicenseUi(), active), {
      type: "paid-result",
      feature: "support",
      open: true,
      message: active.message,
    });
    const next = applyStatus(open, refunded);
    expect(next.unlockedPanel).toBeNull();
    expect(next.view?.reason).toBe("refunded");
  });
});
