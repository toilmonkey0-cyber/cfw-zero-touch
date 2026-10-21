import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  SUPPORT_BUTTON_LABEL,
  SUPPORT_COPY,
  SupportFileControl,
  saveSupportFile,
  type SupportInvoke,
} from "./supportFile";

describe("Save support file", () => {
  it("shows the button and plain-language copy", () => {
    const html = renderToStaticMarkup(
      createElement(SupportFileControl, {
        disabled: false,
        note: "",
        onSave: () => undefined,
      }),
    );
    expect(html).toContain(SUPPORT_BUTTON_LABEL);
    expect(html).toContain("Save support file");
    expect(html).toContain(SUPPORT_COPY);
    expect(SUPPORT_COPY.toLowerCase()).toContain("zip");
    expect(SUPPORT_COPY.toLowerCase()).toContain("game");
    expect(SUPPORT_COPY.toLowerCase()).toContain("license key");
    expect(SUPPORT_COPY.toLowerCase()).toContain("email");
    expect(SUPPORT_COPY.toLowerCase()).toContain("no license");
    const button = html.match(/<button\b[^>]*>/)?.[0] ?? "";
    expect(button).toContain("type=\"button\"");
    expect(button).not.toContain("disabled");
  });

  it("saves the zip without waiting on a license", async () => {
    const calls: string[] = [];
    const invoke: SupportInvoke = async <T>(command: string, args?: Record<string, unknown>) => {
      calls.push(command);
      if (
        command === "license_status" ||
        command === "activate_license" ||
        command === "deactivate_license" ||
        command === "paid_feature"
      ) {
        await new Promise(() => {});
      }
      if (command === "save_support_bundle") {
        expect(args).toEqual({
          destination: "C:\\support\\cfw-card-studio-support.zip",
          library: "D:\\Roms",
        });
        return undefined as T;
      }
      throw new Error(`unexpected ${command}`);
    };

    const outcome = await Promise.race([
      saveSupportFile(
        invoke,
        async () => "C:\\support\\cfw-card-studio-support.zip",
        "D:\\Roms",
      ),
      new Promise<never>((_resolve, reject) => {
        setTimeout(() => reject(new Error("support file waited on licensing")), 400);
      }),
    ]);

    expect(outcome).toBe("saved");
    expect(calls).toEqual(["save_support_bundle"]);
  });

  it("does not ask the app to write a file when the save dialog is cancelled", async () => {
    const calls: string[] = [];
    const invoke: SupportInvoke = async (command: string) => {
      calls.push(command);
      throw new Error(`unexpected ${command}`);
    };
    const outcome = await saveSupportFile(invoke, async () => null, "");
    expect(outcome).toBe("cancelled");
    expect(calls).toEqual([]);
  });
});
