/** Site helpers shared by the Sites page, the site dialog and quick connect. */
import type { FtpsMode } from './ipc/bindings/FtpsMode';
import type { Protocol } from './ipc/bindings/Protocol';
import type { Site } from './ipc/bindings/Site';
import type { SiteColor } from './ipc/bindings/SiteColor';
import type { SiteInput } from './ipc/bindings/SiteInput';

export const PROTOCOL_LABEL: Record<Protocol, string> = { sftp: 'SFTP', ftp: 'FTP', ftps: 'FTPS' };

/** 22 for SFTP, 21 for FTP and explicit FTPS, 990 for implicit FTPS. */
export function defaultPort(protocol: Protocol, ftpsMode: FtpsMode = 'explicit'): number {
  if (protocol === 'sftp') return 22;
  if (protocol === 'ftps' && ftpsMode === 'implicit') return 990;
  return 21;
}

export const SITE_COLORS: readonly SiteColor[] = [
  'blue',
  'green',
  'teal',
  'purple',
  'pink',
  'red',
  'orange',
  'yellow',
  'gray',
];

/** The CSS colour of an avatar (tokens in styles/tokens.css). */
export function siteColorVar(color: SiteColor | null): string {
  return `var(--gem-site-${color ?? 'gray'})`;
}

export function siteInitial(name: string): string {
  const initial = Array.from(name.trim())[0];
  return initial ? initial.toLocaleUpperCase() : '?';
}

/** `deploy@203.0.113.10` (with `:port` when it is not the default). */
export function siteAddress(
  site: Pick<SiteInput, 'user' | 'host' | 'port' | 'protocol' | 'ftpsMode'>,
) {
  const port = site.port !== defaultPort(site.protocol, site.ftpsMode) ? `:${site.port}` : '';
  return `${site.user ? `${site.user}@` : ''}${site.host}${port}`;
}

export function siteToInput(site: Site): SiteInput {
  return {
    name: site.name,
    protocol: site.protocol,
    host: site.host,
    port: site.port,
    user: site.user,
    auth: site.auth,
    keyPath: site.keyPath,
    ftpsMode: site.ftpsMode,
    passive: site.passive,
    initialPath: site.initialPath,
    color: site.color,
  };
}

export function emptySiteInput(): SiteInput {
  return {
    name: '',
    protocol: 'sftp',
    host: '',
    port: 22,
    user: '',
    auth: 'password',
    keyPath: null,
    ftpsMode: 'explicit',
    passive: true,
    initialPath: null,
    color: null,
  };
}

export function matchesSite(site: Site, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return [site.name, site.host, site.user, PROTOCOL_LABEL[site.protocol]].some((field) =>
    field.toLowerCase().includes(needle),
  );
}
