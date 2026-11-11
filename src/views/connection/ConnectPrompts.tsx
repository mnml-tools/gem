/** Dialogs of the connection flow: password/passphrase, trust a server, identity changed. */
import { Button, Dialog, Fields } from '@mnml-tools/ui';
import { KeyRound, ShieldAlert } from 'lucide-react';
import { useId, useState } from 'react';
import type { HostKeyInfo } from '@/lib/ipc/bindings/HostKeyInfo';
import { useAppStatus } from '@/lib/query/hooks';
import { common } from '@/strings/common';
import { connectionStrings as t } from '@/strings/connection';
import type { ConnectionTab } from '@/state/workspace';

type SecretProps = {
  tab: ConnectionTab;
  onSubmit: (secret: string, remember: boolean) => void;
  onCancel: () => void;
};

export function SecretDialog({ tab, onSubmit, onCancel }: SecretProps) {
  const [secret, setSecret] = useState('');
  const [remember, setRemember] = useState(false);
  const passphrase = tab.auth === 'key';
  const saved = tab.target.kind === 'site';
  const keyring = useAppStatus().data?.keyringAvailable ?? true;
  const canRemember = saved && keyring;
  const hintId = useId();
  const messageId = useId();
  return (
    <Dialog
      title={t.secret.title(tab.label)}
      closeLabel={common.cancel}
      onDismiss={onCancel}
      describedBy={messageId}
      className="gem-prompt-dialog"
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(secret, canRemember && remember);
        }}
      >
        <p id={messageId} className="gem-dialog-text">
          {passphrase ? t.secret.messagePassphrase : t.secret.message(tab.user, tab.host)}
        </p>
        <Fields>
          <label className="mnml-field-full">
            {passphrase ? t.secret.passphrase : t.secret.password}
            <input
              type="password"
              data-autofocus
              autoComplete="off"
              value={secret}
              onChange={(event) => setSecret(event.target.value)}
            />
          </label>
          <label className="mnml-field-full gem-check">
            <input
              type="checkbox"
              checked={canRemember && remember}
              disabled={!canRemember}
              aria-describedby={keyring ? undefined : hintId}
              onChange={(event) => setRemember(event.target.checked)}
            />
            <span>{saved ? t.secret.remember : t.secret.rememberUnsaved}</span>
          </label>
          {!keyring && (
            <span id={hintId} className="mnml-field-full gem-field-hint">
              {common.keyringUnavailable}
            </span>
          )}
        </Fields>
        <div className="mnml-dialog-actions">
          <Button type="button" onClick={onCancel}>
            {common.cancel}
          </Button>
          <Button type="submit" variant="primary">
            <KeyRound size={13} aria-hidden />
            {t.secret.connect}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

export function KeyFacts({ hostKey, changed }: { hostKey: HostKeyInfo; changed?: boolean }) {
  return (
    <dl className="gem-key-facts">
      <dt>{t.hostKey.host}</dt>
      <dd>
        {hostKey.host}:{hostKey.port}
      </dd>
      <dt>{t.hostKey.keyType}</dt>
      <dd>{hostKey.keyType}</dd>
      {changed ? (
        <>
          <dt>{t.hostKeyChanged.previous}</dt>
          <dd>{hostKey.previousFingerprint ?? '—'}</dd>
          <dt>{t.hostKeyChanged.current}</dt>
          <dd>{hostKey.fingerprint}</dd>
        </>
      ) : (
        <>
          <dt>{t.hostKey.fingerprint}</dt>
          <dd>{hostKey.fingerprint}</dd>
        </>
      )}
    </dl>
  );
}

type TrustProps = {
  hostKey: HostKeyInfo;
  tls: boolean;
  onTrust: () => void;
  onCancel: () => void;
};

export function TrustDialog({ hostKey, tls, onTrust, onCancel }: TrustProps) {
  const messageId = useId();
  return (
    <Dialog
      title={tls ? t.hostKey.titleTls : t.hostKey.titleSsh}
      role="alertdialog"
      describedBy={messageId}
      onDismiss={onCancel}
      className="gem-prompt-dialog"
    >
      <div className="gem-dialog-body">
        <p id={messageId} className="gem-dialog-text">
          {tls ? t.hostKey.messageTls(hostKey.host) : t.hostKey.messageSsh(hostKey.host)}
        </p>
        <KeyFacts hostKey={hostKey} />
      </div>
      <div className="mnml-dialog-actions">
        <Button type="button" data-autofocus onClick={onCancel}>
          {common.cancel}
        </Button>
        <Button type="button" variant="primary" onClick={onTrust}>
          {tls ? t.hostKey.trustCertificate : t.hostKey.trust}
        </Button>
      </div>
    </Dialog>
  );
}

type ChangedProps = {
  hostKey: HostKeyInfo | null;
  host: string;
  canOpenSite: boolean;
  onOpenSite: () => void;
  onDismiss: () => void;
};

/** Blocking: there is deliberately no way to connect from here (plan §3.6). */
export function HostKeyChangedDialog({
  hostKey,
  host,
  canOpenSite,
  onOpenSite,
  onDismiss,
}: ChangedProps) {
  const messageId = useId();
  return (
    <Dialog
      title={
        <span className="gem-dialog-title-danger">
          <ShieldAlert size={14} aria-hidden />
          {t.hostKeyChanged.title}
        </span>
      }
      role="alertdialog"
      describedBy={messageId}
      onDismiss={onDismiss}
      className="gem-prompt-dialog"
    >
      <div className="gem-dialog-body">
        <p id={messageId} className="gem-dialog-text">
          {t.hostKeyChanged.message(hostKey?.host ?? host)}
        </p>
        {hostKey && <KeyFacts hostKey={hostKey} changed />}
        <p className="gem-dialog-hint">{t.hostKeyChanged.howTo}</p>
      </div>
      <div className="mnml-dialog-actions">
        {canOpenSite && (
          <Button type="button" onClick={onOpenSite}>
            {t.hostKeyChanged.openSite}
          </Button>
        )}
        <Button type="button" data-autofocus onClick={onDismiss}>
          {common.close}
        </Button>
      </div>
    </Dialog>
  );
}
