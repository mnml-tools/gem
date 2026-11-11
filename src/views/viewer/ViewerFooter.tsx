/**
 * The file tab footer (plan §3.4): language picker, encoding, line endings, size, cursor
 * position, wrap, and "Read-only: <reason>". Mono, like the status bar.
 */
import { Separator } from '@mnml-tools/ui';
import { Lock, WrapText } from 'lucide-react';
import { formatEncoding, formatSize, LINE_ENDING_LABEL } from '@/lib/format';
import { isLanguageId, LANGUAGE_OPTIONS, type LanguageId } from '@/lib/languages';
import { keys, SHORTCUTS } from '@/lib/shortcuts';
import type { FileTab } from '@/state/fileTabs';
import { viewerStrings as t } from '@/strings/viewer';

export type ReadOnlyLabel = keyof typeof t.readOnlyReason;

type Props = {
  file: FileTab;
  cursor: { line: number; column: number } | null;
  readOnly: ReadOnlyLabel | null;
  wrap: boolean;
  onToggleWrap: () => void;
  onLanguage: (language: LanguageId) => void;
};

export function ViewerFooter({ file, cursor, readOnly, wrap, onToggleWrap, onLanguage }: Props) {
  const meta = file.meta;
  const text = file.status === 'ready' && meta && !meta.binary;
  const ending = meta ? LINE_ENDING_LABEL[meta.lineEnding] : '';
  // The facts after the language picker, joined by the kit's separator (none before the first).
  const facts: { key: string; text: string }[] = [];
  if (meta && text) {
    facts.push({ key: 'encoding', text: formatEncoding(meta.encoding, meta.bom) });
    facts.push({ key: 'endings', text: meta.mixedLineEndings ? t.footer.mixed(ending) : ending });
  }
  if (meta || file.knownSize != null) {
    facts.push({ key: 'size', text: formatSize(meta?.size ?? file.knownSize) });
  }
  if (cursor && text)
    facts.push({ key: 'cursor', text: t.footer.position(cursor.line, cursor.column) });
  return (
    <footer className="gem-viewer-footer" aria-label={t.footer.label}>
      <div className="gem-viewer-footer-start">
        <select
          className="gem-language-picker"
          aria-label={t.footer.language}
          value={file.language}
          disabled={!text}
          onChange={(event) => {
            if (isLanguageId(event.target.value)) onLanguage(event.target.value);
          }}
        >
          {LANGUAGE_OPTIONS.map((option) => (
            <option key={option.id} value={option.id}>
              {option.label}
            </option>
          ))}
        </select>
        <span className="gem-viewer-facts">
          {facts.map((fact, index) => (
            <span key={fact.key}>
              {index > 0 && <Separator />}
              {fact.text}
            </span>
          ))}
        </span>
      </div>
      <div className="gem-viewer-footer-end">
        {readOnly && (
          <span className="gem-read-only">
            <Lock size={11} aria-hidden />
            {t.footer.readOnly}: {t.readOnlyReason[readOnly]}
          </span>
        )}
        {text && (
          <button
            type="button"
            className="mnml-button gem-footer-button"
            aria-pressed={wrap}
            title={`${t.footer.toggleWrap} (${keys(SHORTCUTS.wrap)})`}
            onClick={onToggleWrap}
          >
            <WrapText size={12} aria-hidden />
            {wrap ? t.footer.wrapOn : t.footer.wrapOff}
          </button>
        )}
      </div>
    </footer>
  );
}
