/** "Keyboard shortcuts" (Mod+/): the help table of `lib/shortcuts.ts`. */
import { Button, Dialog } from '@mnml-tools/ui';
import { SHORTCUT_HELP } from '@/lib/shortcuts';
import { common } from '@/strings/common';
import { helpStrings as t } from '@/strings/settings';

export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  return (
    <Dialog
      title={t.title}
      closeLabel={common.close}
      onDismiss={onClose}
      className="gem-help-dialog"
    >
      <div className="gem-help">
        {SHORTCUT_HELP.map((section) => (
          <section key={section.title}>
            <h3>{section.title}</h3>
            <dl>
              {section.items.map((item) => (
                <div key={item.label}>
                  <dt>{item.label}</dt>
                  <dd>
                    <kbd className="mnml-search-shortcut">{item.keys}</kbd>
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ))}
      </div>
      <div className="mnml-dialog-actions">
        <Button data-autofocus onClick={onClose}>
          {common.done}
        </Button>
      </div>
    </Dialog>
  );
}
