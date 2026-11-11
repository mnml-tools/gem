/** Small dialogs of the file list: a name (new folder, new file, rename) and permissions. */
import { Button, Dialog, Fields, Message, Spinner } from '@mnml-tools/ui';
import { useState } from 'react';
import { formatMode, formatOctal, parseOctal } from '@/lib/format';
import type { Entry } from '@/lib/ipc/bindings/Entry';
import { nameProblem } from '@/lib/paths';
import { common } from '@/strings/common';
import { connectionStrings } from '@/strings/connection';

const t = connectionStrings.dialogs;

type NameProps = {
  title: string;
  action: string;
  initial?: string;
  /** Names already in the folder (a clash is refused before asking the server). */
  taken: ReadonlySet<string>;
  onSubmit: (name: string) => Promise<string | null>;
  onCancel: () => void;
};

/** Asks for a file or folder name; `onSubmit` resolves to an error message or `null`. */
export function NameDialog({ title, action, initial = '', taken, onSubmit, onCancel }: NameProps) {
  const [name, setName] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    const problem = nameProblem(name);
    if (problem) return setError(t.nameErrors[problem]);
    if (name !== initial && taken.has(name)) return setError(t.nameErrors.exists);
    if (name === initial) return onCancel();
    setBusy(true);
    const failure = await onSubmit(name);
    setBusy(false);
    if (failure) setError(failure);
  };

  return (
    <Dialog
      title={title}
      closeLabel={common.cancel}
      dismissible={!busy}
      onDismiss={onCancel}
      className="gem-small-dialog"
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <Fields>
          <label className="mnml-field-full">
            {t.name}
            <input
              data-autofocus
              value={name}
              spellCheck={false}
              autoComplete="off"
              aria-invalid={error ? true : undefined}
              onFocus={(event) => {
                // Select the name without its extension, as file managers do.
                const dot = event.target.value.lastIndexOf('.');
                event.target.setSelectionRange(0, dot > 0 ? dot : event.target.value.length);
              }}
              onChange={(event) => {
                setName(event.target.value);
                setError(null);
              }}
            />
          </label>
        </Fields>
        {error && <Message tone="error">{error}</Message>}
        <div className="mnml-dialog-actions">
          <Button type="button" disabled={busy} onClick={onCancel}>
            {common.cancel}
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {busy && <Spinner size={13} />}
            {action}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

const GROUPS = [
  { label: t.chmodOwner, shift: 6 },
  { label: t.chmodGroup, shift: 3 },
  { label: t.chmodOther, shift: 0 },
] as const;
const BITS = [
  { label: t.chmodRead, bit: 4 },
  { label: t.chmodWrite, bit: 2 },
  { label: t.chmodExecute, bit: 1 },
] as const;

type ChmodProps = {
  entry: Entry;
  onSubmit: (mode: number) => Promise<string | null>;
  onCancel: () => void;
};

/** rwx checkboxes and the octal value, kept in sync. Special bits are preserved. */
export function ChmodDialog({ entry, onSubmit, onCancel }: ChmodProps) {
  const [mode, setMode] = useState((entry.mode ?? 0o644) & 0o7777);
  const [octal, setOctal] = useState(formatOctal(mode));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const setFromBits = (next: number) => {
    setMode(next);
    setOctal(formatOctal(next));
    setError(null);
  };

  const submit = async () => {
    const parsed = parseOctal(octal);
    if (parsed == null) return setError(t.chmodInvalid);
    setBusy(true);
    const failure = await onSubmit(parsed);
    setBusy(false);
    if (failure) setError(failure);
  };

  return (
    <Dialog
      title={t.chmodTitle(entry.name)}
      closeLabel={common.cancel}
      dismissible={!busy}
      onDismiss={onCancel}
      className="gem-small-dialog"
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <table className="gem-chmod">
          <thead>
            <tr>
              <td />
              {BITS.map((bit) => (
                <th key={bit.bit} scope="col">
                  {bit.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {GROUPS.map((group) => (
              <tr key={group.shift}>
                <th scope="row">{group.label}</th>
                {BITS.map((bit) => {
                  const mask = bit.bit << group.shift;
                  return (
                    <td key={bit.bit}>
                      <input
                        type="checkbox"
                        aria-label={`${group.label}: ${bit.label}`}
                        checked={(mode & mask) !== 0}
                        onChange={(event) =>
                          setFromBits(event.target.checked ? mode | mask : mode & ~mask)
                        }
                      />
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
        <Fields>
          <label>
            {t.chmodOctal}
            <input
              data-autofocus
              value={octal}
              inputMode="numeric"
              maxLength={4}
              aria-invalid={error ? true : undefined}
              onChange={(event) => {
                setOctal(event.target.value);
                const parsed = parseOctal(event.target.value);
                if (parsed != null) setMode(parsed);
                setError(null);
              }}
            />
          </label>
          <div className="gem-chmod-preview" aria-live="polite">
            {formatMode(mode, entry.kind)}
          </div>
        </Fields>
        {error && <Message tone="error">{error}</Message>}
        <div className="mnml-dialog-actions">
          <Button type="button" disabled={busy} onClick={onCancel}>
            {common.cancel}
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {busy && <Spinner size={13} />}
            {t.chmodApply}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
