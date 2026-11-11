/**
 * New / edit site (plan §3.2): protocol, host, port (default per protocol until typed), user,
 * authentication, secret, key file, TLS mode, passive mode, initial path, name and colour.
 * "Test connection" shows the result and the server conversation inline.
 */
import { Button, Dialog, Fields, Message, Segmented, Spinner, StatusDot } from '@mnml-tools/ui';
import { useQueryClient } from '@tanstack/react-query';
import { Check, FolderOpen, Plus, ShieldOff } from 'lucide-react';
import { useEffect, useId, useRef, useState, type CSSProperties } from 'react';
import { formatClock } from '@/lib/format';
import { api } from '@/lib/ipc/api';
import type { HostKeyInfo } from '@/lib/ipc/bindings/HostKeyInfo';
import type { Protocol } from '@/lib/ipc/bindings/Protocol';
import type { Site } from '@/lib/ipc/bindings/Site';
import type { SiteInput } from '@/lib/ipc/bindings/SiteInput';
import type { TestResult } from '@/lib/ipc/bindings/TestResult';
import { errorMessage, sentence } from '@/lib/errors';
import { useAppStatus } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { PROTOCOL_LABEL, SITE_COLORS, siteColorVar } from '@/lib/sites';
import { common } from '@/strings/common';
import { siteDialogStrings as t } from '@/strings/sites';
import { KeyFacts } from '../connection/ConnectPrompts';
import {
  formFromInput,
  formFromSite,
  needsSecretField,
  newForm,
  secretForCreate,
  secretForTest,
  secretForUpdate,
  toSiteInput,
  validateSite,
  withProtocol,
  type SiteForm,
  type SiteFormErrors,
} from './siteForm';

type Props = {
  /** The site to edit, or `null` for a new one. */
  site: Site | null;
  /** Prefill for a new site (quick connect "Save as site"). */
  initial?: SiteInput;
  onSaved: (site: Site) => void;
  onCancel: () => void;
};

type TestState =
  | { kind: 'idle' }
  | { kind: 'running' }
  /** The server answered: `result.ok`, or `result.error` with the conversation. */
  | { kind: 'done'; result: TestResult }
  /** The test could not run (invalid input, storage, trusting the key failed). */
  | { kind: 'failed'; message: string };

const PROTOCOLS: Protocol[] = ['sftp', 'ftp', 'ftps'];

export function SiteDialog({ site, initial, onSaved, onCancel }: Props) {
  const queryClient = useQueryClient();
  const [form, setForm] = useState<SiteForm>(() =>
    site ? formFromSite(site) : initial ? formFromInput(initial) : newForm(),
  );
  const [errors, setErrors] = useState<SiteFormErrors>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [test, setTest] = useState<TestState>({ kind: 'idle' });
  const [notice, setNotice] = useState<string | null>(null);
  const secretInput = useRef<HTMLInputElement>(null);
  const [focusSecret, setFocusSecret] = useState(0);
  useEffect(() => {
    if (focusSecret) secretInput.current?.focus();
  }, [focusSecret]);
  const busy = saving || test.kind === 'running';
  const hasSecret = site?.hasSecret ?? false;
  // Without the OS keyring nothing can be remembered: Gem asks when connecting.
  const keyring = useAppStatus().data?.keyringAvailable ?? true;
  const keyringHintId = useId();
  const stored = keyring ? form : { ...form, remember: false };

  const change = (patch: Partial<SiteForm>) => {
    setForm((current) => ({ ...current, ...patch }));
    setTest({ kind: 'idle' });
    setSaveError(null);
  };

  const check = (): boolean => {
    const found = validateSite(form, t.errors);
    setErrors(found);
    return Object.keys(found).length === 0;
  };

  const save = async () => {
    if (!check()) return;
    setSaving(true);
    setSaveError(null);
    try {
      const input = toSiteInput(form);
      const saved = site
        ? await api.updateSite(site.id, input, secretForUpdate(stored, hasSecret))
        : await api.createSite(input, secretForCreate(stored));
      await queryClient.invalidateQueries({ queryKey: queryKeys.sites });
      onSaved(saved);
    } catch (cause) {
      setSaveError(errorMessage(cause));
    } finally {
      setSaving(false);
    }
  };

  const runTest = async () => {
    if (!check()) return;
    setTest({ kind: 'running' });
    try {
      const result = await api.testConnection(
        toSiteInput(form),
        secretForTest(form),
        site?.id ?? null,
      );
      setTest({ kind: 'done', result });
      if (result.error?.code === 'secret_required') {
        // Ask for the password (or passphrase) right where it goes.
        if (hasSecret) setForm((current) => ({ ...current, secretMode: 'change' }));
        setFocusSecret((value) => value + 1);
      }
    } catch (cause) {
      setTest({ kind: 'failed', message: errorMessage(cause) });
    }
  };

  const trustAndTest = async (key: HostKeyInfo) => {
    try {
      await api.trustHostKey(key);
      await runTest();
    } catch (cause) {
      setTest({ kind: 'failed', message: errorMessage(cause) });
    }
  };

  const forgetHostKey = async () => {
    try {
      const forgot = await api.forgetHostKey(form.host.trim(), Number(form.port));
      setNotice(forgot ? t.forgotHostKey : t.noHostKey);
    } catch (cause) {
      setNotice(errorMessage(cause));
    }
  };

  const chooseKey = async () => {
    try {
      const path = await api.chooseKeyFile();
      if (path) change({ keyPath: path });
    } catch (cause) {
      setSaveError(errorMessage(cause));
    }
  };

  const secretLabel = form.protocol === 'sftp' && form.auth === 'key' ? t.passphrase : t.password;
  const showSecretInput = needsSecretField(form) && (!hasSecret || form.secretMode === 'change');

  return (
    <Dialog
      title={site ? t.editTitle : t.newTitle}
      closeLabel={common.close}
      dismissible={!busy}
      onDismiss={onCancel}
      className="gem-site-dialog"
    >
      <form
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <Segmented label={t.protocol} className="gem-protocol-picker">
          {PROTOCOLS.map((protocol) => (
            <button
              key={protocol}
              type="button"
              aria-pressed={form.protocol === protocol}
              onClick={() => change(withProtocol(form, protocol))}
            >
              {PROTOCOL_LABEL[protocol]}
            </button>
          ))}
        </Segmented>
        {form.protocol === 'ftp' && <p className="gem-form-notice">{t.insecureNotice}</p>}
        <Fields className="gem-site-fields">
          <label className="gem-field-host">
            {t.host}
            <input
              data-autofocus
              value={form.host}
              placeholder={t.hostPlaceholder}
              spellCheck={false}
              autoComplete="off"
              aria-invalid={errors.host ? true : undefined}
              onChange={(event) => change({ host: event.target.value })}
            />
            {errors.host && <span className="gem-field-error">{errors.host}</span>}
          </label>
          <label>
            {t.port}
            <input
              value={form.port}
              inputMode="numeric"
              aria-invalid={errors.port ? true : undefined}
              onChange={(event) => change({ port: event.target.value, portEdited: true })}
            />
            {errors.port && <span className="gem-field-error">{errors.port}</span>}
          </label>
          <label>
            {t.user}
            <input
              value={form.user}
              placeholder={form.protocol === 'sftp' ? '' : t.userPlaceholderFtp}
              spellCheck={false}
              autoComplete="off"
              onChange={(event) => change({ user: event.target.value })}
            />
          </label>
          {form.protocol === 'sftp' ? (
            <div className="gem-field">
              <span>{t.auth}</span>
              <Segmented label={t.auth}>
                {(['password', 'key', 'agent'] as const).map((auth) => (
                  <button
                    key={auth}
                    type="button"
                    aria-pressed={form.auth === auth}
                    onClick={() => change({ auth })}
                  >
                    {auth === 'password'
                      ? t.authPassword
                      : auth === 'key'
                        ? t.authKey
                        : t.authAgent}
                  </button>
                ))}
              </Segmented>
            </div>
          ) : (
            <div className="gem-field">
              <span>{t.ftpsMode}</span>
              {form.protocol === 'ftps' ? (
                <Segmented label={t.ftpsMode}>
                  {(['explicit', 'implicit'] as const).map((mode) => (
                    <button
                      key={mode}
                      type="button"
                      aria-pressed={form.ftpsMode === mode}
                      onClick={() => change(withProtocol(form, 'ftps', mode))}
                    >
                      {mode === 'explicit' ? t.ftpsExplicit : t.ftpsImplicit}
                    </button>
                  ))}
                </Segmented>
              ) : (
                <span className="gem-field-static">—</span>
              )}
            </div>
          )}
          {form.protocol === 'sftp' && form.auth === 'key' && (
            <div className="gem-field mnml-field-full">
              <span>{t.keyPath}</span>
              <div className="gem-key-picker">
                <span className="gem-key-path" title={form.keyPath ?? undefined}>
                  {form.keyPath ?? t.noKey}
                </span>
                <Button type="button" onClick={() => void chooseKey()}>
                  <FolderOpen size={13} aria-hidden />
                  {t.chooseKey}
                </Button>
              </div>
              {errors.keyPath && <span className="gem-field-error">{errors.keyPath}</span>}
            </div>
          )}
          {needsSecretField(form) && (
            <div className="gem-field mnml-field-full">
              {hasSecret && (
                <>
                  <span>
                    {form.protocol === 'sftp' && form.auth === 'key'
                      ? t.secretStoredPassphrase
                      : t.secretStored}
                  </span>
                  <Segmented label={secretLabel}>
                    {(['keep', 'change', 'clear'] as const).map((mode) => (
                      <button
                        key={mode}
                        type="button"
                        aria-pressed={form.secretMode === mode}
                        onClick={() => change({ secretMode: mode, secret: '' })}
                      >
                        {mode === 'keep'
                          ? t.secretKeep
                          : mode === 'change'
                            ? t.secretChange
                            : t.secretClear}
                      </button>
                    ))}
                  </Segmented>
                </>
              )}
              {showSecretInput && (
                <label className="gem-secret-field">
                  {secretLabel}
                  <input
                    ref={secretInput}
                    type="password"
                    value={form.secret}
                    autoComplete="new-password"
                    placeholder={form.auth === 'key' ? t.optional : ''}
                    onChange={(event) => change({ secret: event.target.value })}
                  />
                </label>
              )}
              {!hasSecret && (
                <>
                  <label className="gem-check">
                    <input
                      type="checkbox"
                      checked={keyring && form.remember}
                      disabled={!keyring}
                      aria-describedby={keyring ? undefined : keyringHintId}
                      onChange={(event) => change({ remember: event.target.checked })}
                    />
                    <span>{t.rememberPassword}</span>
                  </label>
                  {!keyring && (
                    <span id={keyringHintId} className="gem-field-hint">
                      {common.keyringUnavailable}
                    </span>
                  )}
                </>
              )}
            </div>
          )}
          <label className="mnml-field-full">
            {t.initialPath}
            <input
              value={form.initialPath}
              placeholder={t.initialPathPlaceholder}
              spellCheck={false}
              aria-invalid={errors.initialPath ? true : undefined}
              onChange={(event) => change({ initialPath: event.target.value })}
            />
            {errors.initialPath && <span className="gem-field-error">{errors.initialPath}</span>}
          </label>
          {form.protocol !== 'sftp' && (
            <label className="gem-check mnml-field-full">
              <input
                type="checkbox"
                checked={form.passive}
                onChange={(event) => change({ passive: event.target.checked })}
              />
              <span>{t.passive}</span>
            </label>
          )}
          <label>
            {t.name}
            <input
              value={form.name}
              placeholder={form.host.trim() || t.namePlaceholder}
              onChange={(event) => change({ name: event.target.value })}
            />
          </label>
          <div className="gem-field">
            <span>{t.color}</span>
            <div className="gem-swatches" role="group" aria-label={t.color}>
              {SITE_COLORS.map((color) => (
                <button
                  key={color}
                  type="button"
                  className="mnml-button gem-swatch"
                  aria-label={t.colorName(color)}
                  title={t.colorName(color)}
                  aria-pressed={(form.color ?? 'gray') === color}
                  style={{ '--swatch': siteColorVar(color) } as CSSProperties}
                  onClick={() => change({ color })}
                />
              ))}
            </div>
          </div>
        </Fields>

        {test.kind === 'done' && <TestOutcome result={test.result} onTrust={trustAndTest} />}
        {test.kind === 'failed' && <Message tone="error">{test.message}</Message>}
        {notice && <p className="gem-form-notice">{notice}</p>}
        {saveError && <Message tone="error">{saveError}</Message>}

        <div className="gem-dialog-footer">
          <div>
            <Button type="button" disabled={busy} onClick={() => void runTest()}>
              {test.kind === 'running' ? <Spinner size={13} /> : <StatusDot />}
              {t.test}
            </Button>
            {(site || (test.kind === 'done' && test.result.error?.code === 'host_key_changed')) &&
              form.protocol !== 'ftp' && (
                <Button type="button" disabled={busy} onClick={() => void forgetHostKey()}>
                  <ShieldOff size={13} aria-hidden />
                  {t.forgetHostKey}
                </Button>
              )}
          </div>
          <div>
            <Button type="button" disabled={busy} onClick={onCancel}>
              {common.cancel}
            </Button>
            <Button type="submit" variant="primary" disabled={busy}>
              {saving ? (
                <Spinner size={13} />
              ) : site ? (
                <Check size={13} aria-hidden />
              ) : (
                <Plus size={13} aria-hidden />
              )}
              {site ? t.saveEdit : t.saveNew}
            </Button>
          </div>
        </div>
      </form>
    </Dialog>
  );
}

/** What "Test connection" found: success or the error, and always the conversation. */
function TestOutcome({
  result,
  onTrust,
}: {
  result: TestResult;
  onTrust: (key: HostKeyInfo) => Promise<void>;
}) {
  const error = result.error;
  const trustable =
    error?.hostKey && (error.code === 'host_key_unknown' || error.code === 'certificate_untrusted')
      ? error.hostKey
      : null;
  const changed = error?.code === 'host_key_changed' ? error.hostKey : null;
  return (
    <div className="gem-test-result">
      {result.ok ? (
        <Message tone="success">
          <Check size={13} aria-hidden />
          {t.testOk(result.home ?? '/')}
          {result.serverInfo ? ` · ${result.serverInfo}` : ''}
        </Message>
      ) : (
        <Message tone="error">
          {error ? sentence(error.message) : t.testFailed}
          {trustable && (
            <span className="gem-test-trust">
              <KeyFacts hostKey={trustable} />
              <span className="gem-test-trust-actions">
                <Button type="button" onClick={() => void onTrust(trustable)}>
                  {trustable.keyType === 'x509' ? t.trustCertificateAndTest : t.trustAndTest}
                </Button>
              </span>
            </span>
          )}
          {changed && (
            <span className="gem-test-trust">
              <KeyFacts hostKey={changed} changed />
              <span>{t.hostKeyChangedHint}</span>
            </span>
          )}
        </Message>
      )}
      {result.log.length > 0 && (
        <pre className="gem-test-log" aria-label={t.testLog}>
          {result.log.map((line, index) => (
            <span key={index} data-kind={line.kind}>
              {formatClock(line.ts)} {line.kind.padEnd(8)} {line.text}
              {'\n'}
            </span>
          ))}
        </pre>
      )}
    </div>
  );
}
