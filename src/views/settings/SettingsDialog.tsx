/**
 * Settings (plan §6, Phase 6): download folder, preview window, confirmation threshold,
 * concurrency, keepalive, hidden files, conflict policy. Appearance stays in the kit's
 * ThemeToggle, as in every family app.
 */
import { Button, Dialog, Fields, Message, Spinner } from '@mnml-tools/ui';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { FolderOpen, Keyboard } from 'lucide-react';
import { useState } from 'react';
import { useToast } from '@/components/ui/Toast/context';
import { formatSize } from '@/lib/format';
import { api } from '@/lib/ipc/api';
import type { ConflictPolicy } from '@/lib/ipc/bindings/ConflictPolicy';
import type { Settings } from '@/lib/ipc/bindings/Settings';
import { errorMessage } from '@/lib/errors';
import { DEFAULT_SETTINGS } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { common } from '@/strings/common';
import { useUi } from '@/state/ui';
import { helpStrings, settingsStrings as t } from '@/strings/settings';

const WINDOWS_KIB = [512, 1024, 2048, 4096, 8192, 16384];
const POLICIES: ConflictPolicy[] = ['ask', 'overwrite', 'skip', 'rename'];

type Draft = {
  previewWindowKib: number;
  confirmAboveMib: string;
  transferConcurrency: string;
  keepaliveSecs: string;
  showHidden: boolean;
  conflictPolicy: ConflictPolicy;
};

const toDraft = (settings: Settings): Draft => ({
  previewWindowKib: settings.previewWindowKib,
  confirmAboveMib: String(settings.confirmAboveMib),
  transferConcurrency: String(settings.transferConcurrency),
  keepaliveSecs: String(settings.keepaliveSecs),
  showHidden: settings.showHidden,
  conflictPolicy: settings.conflictPolicy,
});

function wholeNumber(text: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(text.trim())) return null;
  const value = Number(text);
  return value >= min && value <= max ? value : null;
}

export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const queryClient = useQueryClient();
  const toast = useToast();
  const settings = useQuery({ queryKey: queryKeys.settings, queryFn: api.getSettings });
  const current = settings.data ?? DEFAULT_SETTINGS;
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const value = draft ?? toDraft(current);
  const change = (patch: Partial<Draft>) => {
    setDraft({ ...value, ...patch });
    setError(null);
  };

  const confirmAbove = wholeNumber(value.confirmAboveMib, 1, 100_000);
  const concurrency = wholeNumber(value.transferConcurrency, 1, 16);
  const keepalive = wholeNumber(value.keepaliveSecs, 0, 3600);

  const folderAction = async (action: () => Promise<Settings | void>) => {
    try {
      const result = await action();
      if (result) queryClient.setQueryData(queryKeys.settings, result);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  const save = async () => {
    if (confirmAbove == null || concurrency == null || keepalive == null) {
      setError(t.invalid);
      return;
    }
    setBusy(true);
    try {
      const saved = await api.updateSettings({
        ...current,
        previewWindowKib: value.previewWindowKib,
        confirmAboveMib: confirmAbove,
        transferConcurrency: concurrency,
        keepaliveSecs: keepalive,
        showHidden: value.showHidden,
        conflictPolicy: value.conflictPolicy,
      });
      queryClient.setQueryData(queryKeys.settings, saved);
      toast.show({ message: t.saved });
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={t.title}
      closeLabel={common.close}
      dismissible={!busy}
      onDismiss={onClose}
      className="gem-settings-dialog"
    >
      <form
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <Fields>
          <div className="gem-field mnml-field-full">
            <span>{t.downloads}</span>
            <div className="gem-key-picker">
              <span className="gem-key-path" title={current.downloadFolder ?? undefined}>
                {current.downloadFolder ?? t.downloadsDefault}
              </span>
              <Button type="button" onClick={() => void folderAction(api.chooseDownloadFolder)}>
                {t.choose}
              </Button>
              <Button
                type="button"
                disabled={!current.downloadFolder}
                onClick={() =>
                  void folderAction(() => api.updateSettings({ ...current, downloadFolder: null }))
                }
              >
                {t.reset}
              </Button>
              <Button
                type="button"
                aria-label={t.open}
                title={t.open}
                onClick={() => void folderAction(api.openDownloadFolder)}
              >
                <FolderOpen size={13} aria-hidden />
              </Button>
            </div>
          </div>
          <label>
            {t.preview}
            <select
              data-autofocus
              value={value.previewWindowKib}
              onChange={(event) => change({ previewWindowKib: Number(event.target.value) })}
            >
              {[...new Set([...WINDOWS_KIB, value.previewWindowKib])]
                .sort((a, b) => a - b)
                .map((kib) => (
                  <option key={kib} value={kib}>
                    {formatSize(kib * 1024)}
                  </option>
                ))}
            </select>
          </label>
          <label>
            {t.confirmAbove}
            <input
              value={value.confirmAboveMib}
              inputMode="numeric"
              aria-invalid={confirmAbove == null || undefined}
              onChange={(event) => change({ confirmAboveMib: event.target.value })}
            />
          </label>
          <label>
            {t.concurrency}
            <input
              value={value.transferConcurrency}
              inputMode="numeric"
              aria-invalid={concurrency == null || undefined}
              onChange={(event) => change({ transferConcurrency: event.target.value })}
            />
          </label>
          <label>
            {t.keepalive}
            <input
              value={value.keepaliveSecs}
              inputMode="numeric"
              aria-invalid={keepalive == null || undefined}
              onChange={(event) => change({ keepaliveSecs: event.target.value })}
            />
          </label>
          <label>
            {t.conflictPolicy}
            <select
              value={value.conflictPolicy}
              onChange={(event) => change({ conflictPolicy: event.target.value as ConflictPolicy })}
            >
              {POLICIES.map((policy) => (
                <option key={policy} value={policy}>
                  {t.policies[policy]}
                </option>
              ))}
            </select>
          </label>
          <label className="gem-check gem-settings-check">
            <input
              type="checkbox"
              checked={value.showHidden}
              onChange={(event) => change({ showHidden: event.target.checked })}
            />
            <span>{t.showHidden}</span>
          </label>
        </Fields>
        <p className="gem-dialog-hint">
          {t.previewHint} {t.appearanceHint}
        </p>
        {error && <Message tone="error">{error}</Message>}
        <div className="mnml-dialog-actions">
          <Button
            type="button"
            className="gem-actions-start"
            disabled={busy}
            onClick={() => {
              onClose();
              useUi.getState().setHelpOpen(true);
            }}
          >
            <Keyboard size={13} aria-hidden />
            {helpStrings.title}
          </Button>
          <Button type="button" disabled={busy} onClick={onClose}>
            {common.cancel}
          </Button>
          <Button type="submit" variant="primary" disabled={busy || settings.isLoading}>
            {busy && <Spinner size={13} />}
            {t.save}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
