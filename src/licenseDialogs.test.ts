import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LicenseManager, UnlockDialog } from "./LicenseDialogs";

describe("Enter key dialog", () => {
  it("renders the unlock dialog with a refunded result and the key field", () => {
    const html = renderToStaticMarkup(
      createElement(UnlockDialog, {
        featureLabel: "Support",
        draft: "",
        busy: false,
        message: "This license was refunded, so it can't be used.",
        onDraft: () => undefined,
        onActivate: () => undefined,
        onClose: () => undefined,
      }),
    );
    expect(html).toContain('role="dialog"');
    expect(html).toContain("Enter key");
    expect(html).toContain("Support");
    expect(html).toContain("refunded");
    expect(html).toContain("Activate");
    expect(html).toContain('type="password"');
  });

  it("renders the license view with deactivate when the computer is activated", () => {
    const html = renderToStaticMarkup(
      createElement(LicenseManager, {
        message: "License active on this computer.",
        active: true,
        uses: 1,
        busy: false,
        onEnterKey: () => undefined,
        onDeactivate: () => undefined,
        onClose: () => undefined,
      }),
    );
    expect(html).toContain("License active");
    expect(html).toContain("Deactivate this computer");
    expect(html).toContain("1 computer");
  });

  it("offers Enter key from the license view when nothing is activated", () => {
    const html = renderToStaticMarkup(
      createElement(LicenseManager, {
        message: "Enter the license key from your Gumroad receipt to activate this computer.",
        active: false,
        uses: null,
        busy: false,
        onEnterKey: () => undefined,
        onDeactivate: () => undefined,
        onClose: () => undefined,
      }),
    );
    expect(html).toContain("Enter key");
    expect(html).not.toContain("Deactivate this computer");
  });
});
