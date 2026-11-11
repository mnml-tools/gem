/**
 * Lazy CodeMirror support for each `LanguageId`: every language package is a separate chunk,
 * fetched the first time a file of that language is shown. Server configuration formats use
 * Gem's own stream parsers (`streamModes.ts`); a few languages get extra role colouring.
 */
import {
  languageDataProp,
  StreamLanguage,
  syntaxHighlighting,
  syntaxTree,
  type Language,
  type StreamParser,
} from '@codemirror/language';
import { RangeSetBuilder, type Extension } from '@codemirror/state';
import {
  Decoration,
  ViewPlugin,
  type DecorationSet,
  type EditorView,
  type ViewUpdate,
} from '@codemirror/view';
import { tags as t, type Tag } from '@lezer/highlight';
import type { SyntaxRole } from '@/components/ui/CodeView/theme';
import type { LanguageId } from './languages';
import { apache, conf, dockerfile, log, nginx, refineShell, refineToml } from './streamModes';

const legacy = (parser: StreamParser<unknown>) => StreamLanguage.define(parser);

/**
 * Colours a language's own tags with Gem's roles, for that language only. Tags match exactly
 * (no inheritance), so `function(variableName)` keeps its base role when `variableName` gets one.
 */
function languageRoles(language: Language, rules: { tag: Tag; role: SyntaxRole }[]) {
  return syntaxHighlighting({
    style: (tags) => {
      const rule = rules.find(({ tag }) => tags.includes(tag));
      return rule ? `cm-gem-${rule.role}` : null;
    },
    scope: (type) => type.prop(languageDataProp) === language.data,
  });
}

/** Extra roles some languages give their own tags (see `languageRoles`). */
export const LANGUAGE_ROLES: Partial<Record<LanguageId, { tag: Tag; role: SyntaxRole }[]>> = {
  // `$variables` stand out, and bare names (classes, constants) read as types.
  php: [
    { tag: t.variableName, role: 'variable' },
    { tag: t.name, role: 'type' },
  ],
};

const YAML_CONSTANT =
  /^(?:true|false|yes|no|on|off|null|~|[-+]?(?:\d[\d_]*(?:\.\d*)?(?:e[-+]?\d+)?|\.\d+|0x[0-9a-f]+|0o[0-7]+|\.inf|\.nan))$/i;
/** The role of a plain YAML scalar value. */
export function yamlScalarRole(text: string): SyntaxRole {
  return YAML_CONSTANT.test(text) ? 'constant' : 'string';
}

const yamlConstant = Decoration.mark({ class: 'cm-gem-constant' });
const yamlString = Decoration.mark({ class: 'cm-gem-string' });

/**
 * YAML's grammar tags every plain scalar as `content`. Values (not keys) are coloured here:
 * numbers, booleans and null as constants, the rest as strings, as quoted strings are.
 */
const yamlScalars = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = this.build(view);
    }
    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        syntaxTree(update.startState) !== syntaxTree(update.state)
      ) {
        this.decorations = this.build(update.view);
      }
    }
    build(view: EditorView): DecorationSet {
      const builder = new RangeSetBuilder<Decoration>();
      for (const { from, to } of view.visibleRanges) {
        syntaxTree(view.state).iterate({
          from,
          to,
          enter: (node) => {
            if (node.name !== 'Literal' || node.node.parent?.name === 'Key') return;
            const text = view.state.doc.sliceString(node.from, node.to);
            builder.add(
              node.from,
              node.to,
              yamlScalarRole(text) === 'constant' ? yamlConstant : yamlString,
            );
          },
        });
      }
      return builder.finish();
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

/** The CodeMirror extension for a language; plain text is an empty extension. */
export async function loadLanguage(id: LanguageId): Promise<Extension> {
  switch (id) {
    case 'php': {
      const { php, phpLanguage } = await import('@codemirror/lang-php');
      return [php(), languageRoles(phpLanguage, LANGUAGE_ROLES.php ?? [])];
    }
    case 'go':
      return (await import('@codemirror/lang-go')).go();
    case 'yaml':
      return [(await import('@codemirror/lang-yaml')).yaml(), yamlScalars];
    case 'json':
      return (await import('@codemirror/lang-json')).json();
    case 'javascript':
      return (await import('@codemirror/lang-javascript')).javascript();
    case 'jsx':
      return (await import('@codemirror/lang-javascript')).javascript({ jsx: true });
    case 'typescript':
      return (await import('@codemirror/lang-javascript')).javascript({ typescript: true });
    case 'tsx':
      return (await import('@codemirror/lang-javascript')).javascript({
        jsx: true,
        typescript: true,
      });
    case 'html':
      return (await import('@codemirror/lang-html')).html();
    case 'css':
      return (await import('@codemirror/lang-css')).css();
    case 'scss':
      return legacy((await import('@codemirror/legacy-modes/mode/css')).sCSS);
    case 'xml':
      return (await import('@codemirror/lang-xml')).xml();
    case 'sql':
      return (await import('@codemirror/lang-sql')).sql();
    case 'python':
      return (await import('@codemirror/lang-python')).python();
    case 'rust':
      return (await import('@codemirror/lang-rust')).rust();
    case 'markdown':
      return (await import('@codemirror/lang-markdown')).markdown();
    case 'shell':
    case 'makefile':
      return legacy(refineShell((await import('@codemirror/legacy-modes/mode/shell')).shell));
    case 'ini':
    case 'properties':
    case 'dotenv':
      return legacy(conf as StreamParser<unknown>);
    case 'toml':
      return legacy(refineToml((await import('@codemirror/legacy-modes/mode/toml')).toml));
    case 'nginx':
      return legacy(nginx as StreamParser<unknown>);
    case 'dockerfile':
      return legacy(dockerfile as StreamParser<unknown>);
    case 'apache':
      return legacy(apache as StreamParser<unknown>);
    case 'log':
      return legacy(log as StreamParser<unknown>);
    case 'text':
      return [];
  }
}
