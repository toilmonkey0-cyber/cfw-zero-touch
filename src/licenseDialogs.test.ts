import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LicenseManager, PaidFeatureButtons, UnlockDialog } from "./LicenseDialogs";

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

describe("paid feature buttons", () => {
  it("disables Signed builds and Support while a card task is running", () => {
    const html = renderToStaticMarkup(
      createElement(PaidFeatureButtons, {
        cardTaskRunning: true,
        licenseBusy: false,
        onPaid: () => undefined,
      }),
    );
    const buttons = html.match(/<button\b[^>]*>/g) ?? [];
    expect(buttons).toHaveLength(2);
    expect(buttons.every((tag) => tag.includes("disabled"))).toBe(true);
    expect(html).toContain("Signed builds");
    expect(html).toContain("Support");
  });

  it("leaves Signed builds and Support available when no card task is running", () => {
    const html = renderToStaticMarkup(
      createElement(PaidFeatureButtons, {
        cardTaskRunning: false,
        licenseBusy: false,
        onPaid: () => undefined,
      }),
    );
    const buttons = html.match(/<button\b[^>]*>/g) ?? [];
    expect(buttons).toHaveLength(2);
    expect(buttons.some((tag) => tag.includes("disabled"))).toBe(false);
  });

  it("disables Signed builds and Support while a license check is in progress", () => {
    const html = renderToStaticMarkup(
      createElement(PaidFeatureButtons, {
        cardTaskRunning: false,
        licenseBusy: true,
        onPaid: () => undefined,
      }),
    );
    const buttons = html.match(/<button\b[^>]*>/g) ?? [];
    expect(buttons).toHaveLength(2);
    expect(buttons.every((tag) => tag.includes("disabled"))).toBe(true);
  });
});
