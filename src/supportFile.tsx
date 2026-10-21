import type { ReactElement } from "react";

export const SUPPORT_BUTTON_LABEL = "Save support file";

export const SUPPORT_COPY =
  "Saves one zip you can send to support. It includes the app version, the operating system on this computer, recent notes, the latest card test, and whether a license is turned on. Game names, your folders, the license key, and email addresses are left out. Saving this file needs no license.";

export type SupportInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export async function saveSupportFile(
  invoke: SupportInvoke,
  pickSavePath: () => Promise<string | null>,
  library: string,
): Promise<"saved" | "cancelled"> {
  const selected = await pickSavePath();
  if (selected === null) {
    return "cancelled";
  }
  const destination = selected.trim();
  if (destination.length === 0) {
    return "cancelled";
  }
  const trimmedLibrary = library.trim();
  await invoke<void>("save_support_bundle", {
    destination,
    library: trimmedLibrary.length > 0 ? trimmedLibrary : null,
  });
  return "saved";
}

export function SupportFileControl(props: {
  disabled: boolean;
  note: string;
  onSave: () => void;
}): ReactElement {
  return (
    <section>
      <p className="legal">{SUPPORT_COPY}</p>
      <p>
        <button type="button" disabled={props.disabled} onClick={props.onSave}>
          {SUPPORT_BUTTON_LABEL}
        </button>
      </p>
      {props.note.length > 0 ? <p>{props.note}</p> : null}
    </section>
  );
}
