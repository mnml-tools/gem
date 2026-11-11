/** Preview of sites found by an import (ssh config, FileZilla, a Gem file): pick, then create. */
import { Avatar, Badge, Button, Dialog, Message, Spinner } from '@mnml-tools/ui';
import { useState } from 'react';
import type { SiteInput } from '@/lib/ipc/bindings/SiteInput';
import { PROTOCOL_LABEL, siteAddress, siteColorVar, siteInitial } from '@/lib/sites';
import { common } from '@/strings/common';
import { sitesStrings } from '@/strings/sites';

const t = sitesStrings.importDialog;

type Props = {
  sites: SiteInput[];
  onImport: (sites: SiteInput[]) => Promise<string | null>;
  onCancel: () => void;
};

export function ImportDialog({ sites, onImport, onCancel }: Props) {
  const [chosen, setChosen] = useState<ReadonlySet<number>>(() => new Set(sites.map((_, i) => i)));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const all = chosen.size === sites.length;

  const toggle = (index: number) =>
    setChosen((current) => {
      const next = new Set(current);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });

  return (
    <Dialog
      title={t.title}
      closeLabel={common.cancel}
      dismissible={!busy}
      onDismiss={onCancel}
      className="gem-import-dialog"
    >
      <div className="gem-dialog-body">
        <p className="gem-dialog-text">{sites.length ? t.hint : t.none}</p>
        {sites.length > 0 && (
          <>
            <label className="gem-check gem-import-all">
              <input
                type="checkbox"
                checked={all}
                onChange={() => setChosen(all ? new Set() : new Set(sites.map((_, i) => i)))}
              />
              <span>{t.selectAll}</span>
            </label>
            <ul className="gem-import-list">
              {sites.map((site, index) => (
                <li key={`${site.name}-${index}`}>
                  <label className="gem-check">
                    <input
                      type="checkbox"
                      checked={chosen.has(index)}
                      onChange={() => toggle(index)}
                    />
                    <Avatar color={siteColorVar(site.color)} size="small">
                      {siteInitial(site.name || site.host)}
                    </Avatar>
                    <span className="gem-import-name">{site.name || site.host}</span>
                    <span className="gem-import-address">{siteAddress(site)}</span>
                    <Badge>{PROTOCOL_LABEL[site.protocol]}</Badge>
                  </label>
                </li>
              ))}
            </ul>
          </>
        )}
        {error && <Message tone="error">{error}</Message>}
      </div>
      <div className="mnml-dialog-actions">
        <Button type="button" disabled={busy} onClick={onCancel}>
          {common.cancel}
        </Button>
        <Button
          type="button"
          variant="primary"
          data-autofocus
          disabled={busy || chosen.size === 0}
          onClick={async () => {
            setBusy(true);
            const failure = await onImport(sites.filter((_, index) => chosen.has(index)));
            setBusy(false);
            if (failure) setError(failure);
          }}
        >
          {busy && <Spinner size={13} />}
          {t.confirm(chosen.size)}
        </Button>
      </div>
    </Dialog>
  );
}
