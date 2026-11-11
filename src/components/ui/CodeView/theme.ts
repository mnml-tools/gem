/**
 * CodeView's look, built only from CSS custom properties: the kit's tokens for the chrome and
 * `--gem-syntax-*` (styles/tokens.css: the family code palette, light and dark) for highlighting,
 * so the editor follows the colour mode without JavaScript.
 */
import { HighlightStyle } from '@codemirror/language';
import { EditorView } from '@codemirror/view';
import { tags as t, type Tag } from '@lezer/highlight';

export const syntax = (name: string) => `var(--gem-syntax-${name})`;

/** A colour role of the syntax palette (`--gem-syntax-<role>` in styles/tokens.css). */
export type SyntaxRole =
  | 'keyword'
  | 'string'
  | 'regexp'
  | 'constant'
  | 'comment'
  | 'meta'
  | 'function'
  | 'type'
  | 'tag'
  | 'property'
  | 'variable'
  | 'punctuation'
  | 'invalid'
  | 'link';

type RoleSpec = { role: SyntaxRole; tags: Tag[]; style?: Record<string, string> };

/**
 * Which Lezer tags get which role, for every language. A tag inherits the style of its parent
 * (`t.paren` → `t.bracket`, `t.definition(t.propertyName)` → `t.propertyName`), so only the
 * roots that need a colour are listed. Plain `variableName` stays in text ink here: languages
 * that mark variables explicitly (`$x` in PHP and shell) colour them in `languageSupport.ts`.
 */
export const SYNTAX_ROLES: RoleSpec[] = [
  {
    role: 'keyword',
    tags: [
      t.keyword,
      t.controlKeyword,
      t.moduleKeyword,
      t.operatorKeyword,
      t.definitionKeyword,
      t.modifier,
      t.self,
    ],
  },
  {
    role: 'string',
    tags: [
      t.string,
      t.special(t.string),
      t.character,
      t.attributeValue,
      t.inserted,
      t.docString,
      t.quote,
    ],
  },
  { role: 'regexp', tags: [t.regexp, t.escape] },
  {
    role: 'constant',
    tags: [
      t.number,
      t.bool,
      t.null,
      t.atom,
      t.unit,
      t.color,
      t.constant(t.name),
      t.constant(t.variableName),
    ],
  },
  { role: 'comment', tags: [t.comment], style: { fontStyle: 'italic' } },
  { role: 'meta', tags: [t.meta, t.processingInstruction, t.documentMeta, t.annotation] },
  {
    role: 'function',
    tags: [
      t.function(t.variableName),
      t.function(t.propertyName),
      t.function(t.definition(t.variableName)),
      t.macroName,
    ],
  },
  { role: 'type', tags: [t.typeName, t.className, t.namespace] },
  { role: 'tag', tags: [t.tagName] },
  { role: 'tag', tags: [t.heading], style: { fontWeight: '600' } },
  { role: 'property', tags: [t.attributeName, t.propertyName, t.labelName] },
  { role: 'variable', tags: [t.special(t.variableName), t.local(t.variableName)] },
  {
    role: 'punctuation',
    tags: [t.operator, t.punctuation, t.separator, t.bracket, t.derefOperator, t.contentSeparator],
  },
  { role: 'invalid', tags: [t.invalid, t.deleted] },
  { role: 'link', tags: [t.link, t.url], style: { textDecoration: 'underline' } },
];

export const codeViewHighlight = HighlightStyle.define([
  ...SYNTAX_ROLES.map(({ role, tags, style }) => ({ tag: tags, color: syntax(role), ...style })),
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strong, fontWeight: '600' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
]);

/** Role colours for decorations that languages add themselves (`cm-gem-<role>` classes). */
const roleClasses = Object.fromEntries(
  [...new Set(SYNTAX_ROLES.map((spec) => spec.role))].map((role) => [
    `.cm-gem-${role}`,
    { color: syntax(role) },
  ]),
);

export const codeViewTheme = EditorView.theme({
  ...roleClasses,
  '&': {
    height: '100%',
    color: 'var(--text)',
    backgroundColor: 'var(--background)',
    fontSize: '12px',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': {
    fontFamily: 'var(--mono)',
    lineHeight: '1.6',
    overscrollBehavior: 'contain',
  },
  '.cm-content': { padding: '6px 0', caretColor: 'var(--focus)' },
  '.cm-line': { padding: '0 12px 0 8px' },
  '.cm-cursor, .cm-dropCursor': { borderLeft: '2px solid var(--focus)' },
  '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection':
    { backgroundColor: 'var(--gem-code-selection)' },
  '.cm-selectionBackground': { backgroundColor: 'var(--gem-code-selection-blur)' },
  '.cm-activeLine': { backgroundColor: 'var(--gem-code-active-line)' },
  '.cm-gutters': {
    backgroundColor: 'var(--raised)',
    color: 'var(--faint)',
    border: 'none',
    borderRight: '1px solid var(--border)',
  },
  '.cm-lineNumbers .cm-gutterElement': { padding: '0 8px 0 12px', minWidth: '36px' },
  '.cm-activeLineGutter': {
    backgroundColor: 'var(--gem-code-active-line)',
    color: 'var(--secondary)',
  },
  '.cm-foldGutter .cm-gutterElement': {
    width: '14px',
    padding: '0 2px',
    color: 'var(--faint)',
    cursor: 'pointer',
  },
  '.cm-foldGutter .cm-gutterElement:hover': { color: 'var(--text)' },
  '.cm-foldPlaceholder': {
    margin: '0 2px',
    padding: '0 4px',
    border: '1px solid var(--border)',
    backgroundColor: 'var(--soft)',
    color: 'var(--muted)',
  },
  '&.cm-focused .cm-matchingBracket': {
    backgroundColor: 'var(--gem-code-bracket)',
    outline: '1px solid var(--faint)',
  },
  '&.cm-focused .cm-nonmatchingBracket': { color: 'var(--gem-syntax-invalid)' },
  '.cm-searchMatch': {
    backgroundColor: 'var(--edited-bg)',
    outline: '1px solid var(--edited-accent)',
  },
  '.cm-searchMatch.cm-searchMatch-selected': {
    backgroundColor: 'var(--gem-code-match-selected)',
  },
  '.cm-selectionMatch': { backgroundColor: 'var(--gem-code-selection-blur)' },
  '.cm-indent-guides': {
    backgroundImage: 'linear-gradient(to right, var(--gem-code-indent-guide) 1px, transparent 1px)',
    backgroundSize: 'calc(var(--gem-indent-unit, 4) * 1ch) 100%',
    backgroundRepeat: 'repeat-x',
  },
  // Panels (search, go to line): the kit's soft strip, square fields and buttons.
  '.cm-panels': {
    backgroundColor: 'var(--soft)',
    color: 'var(--text)',
    fontFamily: 'inherit',
    fontSize: '11px',
  },
  '.cm-panels.cm-panels-top': { borderBottom: '1px solid var(--border)' },
  '.cm-panels.cm-panels-bottom': { borderTop: '1px solid var(--border)' },
  '.cm-panel': {
    fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
  },
  '.cm-panel.cm-search': { padding: '6px 28px 6px 8px' },
  '.cm-panel.cm-gotoLine': { padding: '6px 8px' },
  '.cm-panel label': { fontSize: '11px', color: 'var(--secondary)' },
  '.cm-textfield': {
    height: '24px',
    padding: '2px 6px',
    border: '1px solid var(--border)',
    borderRadius: '0',
    backgroundColor: 'var(--background)',
    color: 'var(--text)',
    font: '11px var(--mono)',
  },
  '.cm-textfield:focus': { borderColor: 'var(--focus)', outline: 'none' },
  '.cm-button': {
    minHeight: '24px',
    padding: '2px 8px',
    border: '1px solid var(--border)',
    borderRadius: '0',
    backgroundImage: 'none',
    backgroundColor: 'var(--background)',
    color: 'var(--text)',
    fontSize: '11px',
  },
  '.cm-button:hover': { backgroundColor: 'var(--hover)' },
  '.cm-button:active': { backgroundImage: 'none', backgroundColor: 'var(--hover-strong)' },
  '.cm-panel.cm-search [name=close]': {
    top: '6px',
    right: '6px',
    color: 'var(--icon)',
    fontSize: '16px',
  },
  '.cm-tooltip': {
    border: '1px solid var(--faint)',
    backgroundColor: 'var(--background)',
    boxShadow: '0 3px 10px var(--shadow)',
  },
});
