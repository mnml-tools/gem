/**
 * Manage snippets (ADR 0014): "This site" and "Global" lists, each reorderable with Alt+↑/↓ or
 * the arrow buttons, and an editor for one snippet (name, command, scope, ask before running).
 */
import {
  Button,
  ConfirmDialog,
  Dialog,
  Fields,
  IconButton,
  Message,
  Segmented,
  Spinner,
} from '@mnml-tools/ui';
import { useQueryClient } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, Pencil, Plus, ShieldQuestion, Trash2 } from 'lucide-react';
import { useState, type KeyboardEvent } from 'react';
import { errorMessage } from '@/lib/errors';
import { api } from '@/lib/ipc/api';
import type { Snippet } from '@/lib/ipc/bindings/Snippet';
import { useSnippets } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { commandStrings } from '@/strings/commands';
import { common } from '@/strings/common';

const t = commandStrings.manage;

type Props = { siteId: string | null; siteName: string | null; onClose: () => void };

type Draft = {
  id: string | null;
  name: string;
  command: string;
  global: boolean;
  confirm: boolean;
};

export function SnippetsDialog({ siteId, siteName, onClose }: Props) {
  const queryClient = useQueryClient();
  const snippets = useSnippets(siteId);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [deleting, setDeleting] = useState<Snippet | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const all = snippets.data ?? [];
  const site = all.filter((snippet) => snippet.siteId !== null);
  const global = all.filter((snippet) => snippet.siteId === null);

  const refresh = () => queryClient.invalidateQueries({ queryKey: queryKeys.allSnippets });

  const move = async (list: Snippet[], snippet: Snippet, step: number) => {
    const index = list.findIndex((item) => item.id === snippet.id);
    const target = index + step;
    if (index < 0 || target < 0 || target >= list.length) return;
    const ids = list.map((item) => item.id);
    [ids[index], ids[target]] = [ids[target]!, ids[index]!];
    // Show the new order at once; the server confirms it.
    queryClient.setQueryData<Snippet[]>(queryKeys.snippets(siteId), (current) => {
      if (!current) return current;
      const byId = new Map(current.map((item) => [item.id, item]));
      const moved = ids.map((id) => byId.get(id)!);
      const others = current.filter((item) => !ids.includes(item.id));
      return list[0]?.siteId === null ? [...moved, ...others] : [...others, ...moved];
    });
    try {
      await api.reorderSnippets(ids);
    } catch (cause) {
      setError(errorMessage(cause));
    }
    await refresh();
  };

  const remove = async (snippet: Snippet) => {
    setDeleting(null);
    try {
      await api.deleteSnippet(snippet.id);
    } catch (cause) {
      setError(errorMessage(cause));
    }
    await refresh();
  };

  if (draft) {
    return (
      <SnippetEditor
        draft={draft}
        siteName={siteId ? siteName : null}
        onBack={() => setDraft(null)}
        onSave={async (value) => {
          const input = {
            siteId: value.global ? null : siteId,
            name: value.name.trim(),
            command: value.command,
            confirm: value.confirm,
          };
          if (value.id) await api.updateSnippet(value.id, input);
          else await api.createSnippet(input);
          await refresh();
          setDraft(null);
        }}
      />
    );
  }

  const section = (title: string, hint: string | null, list: Snippet[], global: boolean) => (
    <section className="gem-snippet-section" aria-label={title}>
      <header>
        <h3>{title}</h3>
        {hint && <span>{hint}</span>}
        <Button
          onClick={() => setDraft({ id: null, name: '', command: '', global, confirm: false })}
        >
          <Plus size={12} aria-hidden />
          {t.add}
        </Button>
      </header>
      {list.length === 0 ? (
        <p className="gem-snippet-empty">{t.empty}</p>
      ) : (
        <ul className="gem-snippet-list">
          {list.map((snippet, index) => (
            // eslint-disable-next-line jsx-a11y-x/no-noninteractive-element-interactions
            <li
              key={snippet.id}
              // A focusable row: Alt+↑/↓ reorders it (the buttons do the same with the pointer).
              // eslint-disable-next-line jsx-a11y-x/no-noninteractive-tabindex
              tabIndex={0}
              aria-label={snippet.name}
              data-selected={selected === snippet.id || undefined}
              onFocus={() => setSelected(snippet.id)}
              onKeyDown={(event: KeyboardEvent<HTMLLIElement>) => {
                if (event.target !== event.currentTarget || !event.altKey) return;
                if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
                event.preventDefault();
                const row = event.currentTarget;
                void move(list, snippet, event.key === 'ArrowUp' ? -1 : 1).then(() => row.focus());
              }}
            >
              <span className="gem-snippet-name">{snippet.name}</span>
              <code className="gem-snippet-command" title={snippet.command}>
                {snippet.command}
              </code>
              {snippet.confirm && (
                <span className="gem-snippet-asks" title={t.asks}>
                  <ShieldQuestion size={12} aria-hidden />
                  {t.asks}
                </span>
              )}
              <span className="gem-snippet-actions">
                <IconButton
                  aria-label={t.moveUp(snippet.name)}
                  title={t.moveUp(snippet.name)}
                  disabled={index === 0}
                  onClick={() => void move(list, snippet, -1)}
                >
                  <ArrowUp size={12} aria-hidden />
                </IconButton>
                <IconButton
                  aria-label={t.moveDown(snippet.name)}
                  title={t.moveDown(snippet.name)}
                  disabled={index === list.length - 1}
                  onClick={() => void move(list, snippet, 1)}
                >
                  <ArrowDown size={12} aria-hidden />
                </IconButton>
                <IconButton
                  aria-label={t.edit(snippet.name)}
                  title={t.edit(snippet.name)}
                  onClick={() =>
                    setDraft({
                      id: snippet.id,
                      name: snippet.name,
                      command: snippet.command,
                      global: snippet.siteId === null,
                      confirm: snippet.confirm,
                    })
                  }
                >
                  <Pencil size={12} aria-hidden />
                </IconButton>
                <IconButton
                  aria-label={t.delete(snippet.name)}
                  title={t.delete(snippet.name)}
                  onClick={() => setDeleting(snippet)}
                >
                  <Trash2 size={12} aria-hidden />
                </IconButton>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );

  return (
    <Dialog
      title={t.title}
      closeLabel={common.close}
      onDismiss={onClose}
      className="gem-snippets-dialog"
    >
      <div className="gem-dialog-body gem-snippets-body">
        {snippets.isLoading && <Spinner size={14} />}
        {siteId && section(t.site(siteName ?? ''), null, site, false)}
        {section(t.global, t.globalHint, global, true)}
        <p className="gem-dialog-hint">{t.reorderHint}</p>
        {error && <Message tone="error">{error}</Message>}
      </div>
      <div className="mnml-dialog-actions">
        <Button data-autofocus onClick={onClose}>
          {common.done}
        </Button>
      </div>
      {deleting && (
        <ConfirmDialog
          title={t.deleteTitle}
          message={t.deleteMessage(deleting.name)}
          confirmLabel={t.deleteConfirm}
          cancelLabel={common.cancel}
          onCancel={() => setDeleting(null)}
          onConfirm={() => void remove(deleting)}
        />
      )}
    </Dialog>
  );
}

function SnippetEditor({
  draft,
  siteName,
  onBack,
  onSave,
}: {
  draft: Draft;
  /** The site of the connection tab; `null` when only global snippets can be made. */
  siteName: string | null;
  onBack: () => void;
  onSave: (draft: Draft) => Promise<void>;
}) {
  const [value, setValue] = useState(draft);
  const [errors, setErrors] = useState<{ name?: string; command?: string }>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const change = (patch: Partial<Draft>) => {
    setValue((current) => ({ ...current, ...patch }));
    setErrors({});
    setFailure(null);
  };

  const submit = async () => {
    const found = {
      name: value.name.trim() ? undefined : t.nameRequired,
      command: value.command.trim() ? undefined : t.commandRequired,
    };
    if (found.name || found.command) {
      setErrors(found);
      return;
    }
    setBusy(true);
    try {
      await onSave(value);
    } catch (cause) {
      setFailure(errorMessage(cause));
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={draft.id ? t.editTitle : t.newTitle}
      closeLabel={common.close}
      dismissible={!busy}
      onDismiss={onBack}
      className="gem-snippets-dialog"
    >
      <form
        noValidate
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
              value={value.name}
              placeholder={t.namePlaceholder}
              aria-invalid={errors.name ? true : undefined}
              onChange={(event) => change({ name: event.target.value })}
            />
            {errors.name && <span className="gem-field-error">{errors.name}</span>}
          </label>
          <label className="mnml-field-full">
            {t.command}
            <textarea
              className="gem-snippet-input"
              value={value.command}
              rows={4}
              spellCheck={false}
              placeholder={t.commandPlaceholder}
              aria-invalid={errors.command ? true : undefined}
              onChange={(event) => change({ command: event.target.value })}
            />
            {errors.command && <span className="gem-field-error">{errors.command}</span>}
            <span className="gem-field-hint">{t.dirHint}</span>
          </label>
          {siteName !== null && (
            <div className="gem-field">
              <span>{t.scope}</span>
              <Segmented label={t.scope}>
                <button
                  type="button"
                  aria-pressed={!value.global}
                  onClick={() => change({ global: false })}
                >
                  {t.scopeSite}
                </button>
                <button
                  type="button"
                  aria-pressed={value.global}
                  onClick={() => change({ global: true })}
                >
                  {t.scopeGlobal}
                </button>
              </Segmented>
            </div>
          )}
          <label className="gem-check gem-snippet-confirm">
            <input
              type="checkbox"
              checked={value.confirm}
              onChange={(event) => change({ confirm: event.target.checked })}
            />
            <span>{t.confirm}</span>
          </label>
        </Fields>
        {failure && <Message tone="error">{failure}</Message>}
        <div className="mnml-dialog-actions">
          <Button type="button" disabled={busy} onClick={onBack}>
            {t.back}
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {busy && <Spinner size={13} />}
            {t.save}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
