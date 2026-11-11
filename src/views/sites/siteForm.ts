/** The site dialog's form model: defaults per protocol, validation and the IPC payloads. */
import type { AuthMethod } from '@/lib/ipc/bindings/AuthMethod';
import type { FtpsMode } from '@/lib/ipc/bindings/FtpsMode';
import type { Protocol } from '@/lib/ipc/bindings/Protocol';
import type { SecretUpdate } from '@/lib/ipc/bindings/SecretUpdate';
import type { Site } from '@/lib/ipc/bindings/Site';
import type { SiteColor } from '@/lib/ipc/bindings/SiteColor';
import type { SiteInput } from '@/lib/ipc/bindings/SiteInput';
import { defaultPort, emptySiteInput, siteToInput } from '@/lib/sites';

export type SecretMode = 'keep' | 'change' | 'clear';

export type SiteForm = {
  protocol: Protocol;
  host: string;
  /** Text, so a half-typed port stays editable. */
  port: string;
  /** The port was typed by the user: protocol changes leave it alone. */
  portEdited: boolean;
  user: string;
  auth: AuthMethod;
  keyPath: string | null;
  ftpsMode: FtpsMode;
  passive: boolean;
  initialPath: string;
  name: string;
  color: SiteColor | null;
  secret: string;
  remember: boolean;
  /** Editing a site that has a stored secret. */
  secretMode: SecretMode;
};

export type SiteFormErrors = Partial<Record<'host' | 'port' | 'keyPath' | 'initialPath', string>>;

export function formFromInput(input: SiteInput, edited = false): SiteForm {
  return {
    protocol: input.protocol,
    host: input.host,
    port: String(input.port),
    portEdited: edited || input.port !== defaultPort(input.protocol, input.ftpsMode),
    user: input.user,
    auth: input.auth,
    keyPath: input.keyPath,
    ftpsMode: input.ftpsMode,
    passive: input.passive,
    initialPath: input.initialPath ?? '',
    name: input.name,
    color: input.color,
    secret: '',
    remember: true,
    secretMode: 'keep',
  };
}

export function formFromSite(site: Site): SiteForm {
  return formFromInput(siteToInput(site));
}

export function newForm(): SiteForm {
  return formFromInput(emptySiteInput());
}

/** Switching protocol (or TLS mode) moves the port to its default unless it was typed. */
export function withProtocol(
  form: SiteForm,
  protocol: Protocol,
  ftpsMode = form.ftpsMode,
): SiteForm {
  return {
    ...form,
    protocol,
    ftpsMode,
    // Only SFTP has key and agent authentication.
    auth: protocol === 'sftp' ? form.auth : 'password',
    port: form.portEdited ? form.port : String(defaultPort(protocol, ftpsMode)),
  };
}

/** Whether the form asks for a password (or a key passphrase). */
export function needsSecretField(form: SiteForm): boolean {
  return form.protocol !== 'sftp' || form.auth !== 'agent';
}

export function validateSite(
  form: SiteForm,
  messages: Record<keyof Required<SiteFormErrors>, string>,
): SiteFormErrors {
  const errors: SiteFormErrors = {};
  const host = form.host.trim();
  if (!host || /\s|\/|@/.test(host)) errors.host = messages.host;
  const port = Number(form.port);
  if (!/^\d{1,5}$/.test(form.port.trim()) || port < 1 || port > 65535) errors.port = messages.port;
  if (form.protocol === 'sftp' && form.auth === 'key' && !form.keyPath)
    errors.keyPath = messages.keyPath;
  const path = form.initialPath.trim();
  if (path && !path.startsWith('/') && !path.startsWith('~'))
    errors.initialPath = messages.initialPath;
  return errors;
}

export function toSiteInput(form: SiteForm): SiteInput {
  const host = form.host.trim();
  return {
    name: form.name.trim() || host,
    protocol: form.protocol,
    host,
    port: Number(form.port),
    user: form.user.trim(),
    auth: form.protocol === 'sftp' ? form.auth : 'password',
    keyPath: form.protocol === 'sftp' && form.auth === 'key' ? form.keyPath : null,
    ftpsMode: form.ftpsMode,
    passive: form.passive,
    initialPath: form.initialPath.trim() || null,
    color: form.color,
  };
}

/** The secret for `create_site`: only when typed and "Remember" is on. */
export function secretForCreate(form: SiteForm): string | null {
  return needsSecretField(form) && form.remember && form.secret ? form.secret : null;
}

/** The secret change for `update_site`. */
export function secretForUpdate(form: SiteForm, hasSecret: boolean): SecretUpdate {
  if (!needsSecretField(form)) return hasSecret ? { kind: 'clear' } : { kind: 'keep' };
  if (hasSecret) {
    if (form.secretMode === 'clear') return { kind: 'clear' };
    if (form.secretMode === 'change' && form.secret) return { kind: 'set', value: form.secret };
    return { kind: 'keep' };
  }
  return form.remember && form.secret ? { kind: 'set', value: form.secret } : { kind: 'keep' };
}

/** The secret to try in "Test connection": what was typed, else the stored one (null). */
export function secretForTest(form: SiteForm): string | null {
  return needsSecretField(form) && form.secret ? form.secret : null;
}
