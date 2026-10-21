type UnlockProps = {
  featureLabel: string;
  draft: string;
  busy: boolean;
  message: string;
  onDraft: (value: string) => void;
  onActivate: () => void;
  onClose: () => void;
};

export function UnlockDialog(props: UnlockProps) {
  return (
    <div className="modal-backdrop">
      <form
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="enter-key-title"
        onSubmit={(event) => {
          event.preventDefault();
          if (!props.busy && props.draft.trim().length > 0) props.onActivate();
        }}
      >
        <h2 id="enter-key-title">Enter key</h2>
        <p>
          {props.featureLabel} needs the license key from your Gumroad receipt. Backup, restore,
          card tests, and preparing a ROM card stay available without one.
        </p>
        <label>
          License key
          <input
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={props.draft}
            onChange={(event) => props.onDraft(event.target.value)}
          />
        </label>
        {props.message ? <p>{props.message}</p> : null}
        <div className="row">
          <button type="submit" disabled={props.busy || props.draft.trim().length === 0}>
            Activate
          </button>
          <button type="button" disabled={props.busy} onClick={props.onClose}>
            Cancel
          </button>
        </div>
      </form>
    </div>
  );
}

type ManagerProps = {
  message: string;
  active: boolean;
  uses: number | null;
  busy: boolean;
  onEnterKey: () => void;
  onDeactivate: () => void;
  onClose: () => void;
};

export function LicenseManager(props: ManagerProps) {
  const seats =
    props.uses === null
      ? null
      : props.uses === 1
        ? "In use on 1 computer."
        : `In use on ${props.uses} computers.`;
  return (
    <div className="modal-backdrop">
      <section className="modal" role="dialog" aria-modal="true" aria-labelledby="license-view-title">
        <h2 id="license-view-title">License</h2>
        <p>{props.message}</p>
        {seats ? <p>{seats}</p> : null}
        <div className="row">
          {props.active ? (
            <button type="button" disabled={props.busy} onClick={props.onDeactivate}>
              Deactivate this computer
            </button>
          ) : (
            <button type="button" disabled={props.busy} onClick={props.onEnterKey}>
              Enter key
            </button>
          )}
          <button type="button" disabled={props.busy} onClick={props.onClose}>
            Close
          </button>
        </div>
      </section>
    </div>
  );
}
