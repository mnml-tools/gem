/** Opening connections from the Sites page, quick connect and tab menus. */
import type { Site } from '@/lib/ipc/bindings/Site';
import type { SiteInput } from '@/lib/ipc/bindings/SiteInput';
import { holdSecret } from '@/state/secrets';
import { useWorkspace, type ConnectionMeta } from '@/state/workspace';

export function siteMeta(site: Site): ConnectionMeta {
  return {
    target: { kind: 'site', id: site.id },
    label: site.name,
    protocol: site.protocol,
    insecure: site.protocol === 'ftp',
    color: site.color,
    siteId: site.id,
    user: site.user,
    host: site.host,
    port: site.port,
    auth: site.auth,
  };
}

export function adhocMeta(site: SiteInput): ConnectionMeta {
  return {
    target: { kind: 'adhoc', site },
    label: site.name || site.host,
    protocol: site.protocol,
    insecure: site.protocol === 'ftp',
    color: site.color,
    siteId: null,
    user: site.user,
    host: site.host,
    port: site.port,
    auth: site.auth,
  };
}

/**
 * Opens a connection tab. From a Sites tab, `replaceId` turns that tab into the connection
 * (like a browser's new-tab page); otherwise a new tab opens.
 */
export function connect(meta: ConnectionMeta, replaceId: string | null, secret?: string | null) {
  const id = useWorkspace.getState().openConnection(meta, replaceId);
  if (secret) holdSecret(id, secret);
  return id;
}
