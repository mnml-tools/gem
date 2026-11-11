/**
 * Every v1 language, on the demo server's sample files, must come out multi-coloured: the
 * grammar's tags (plus Gem's per-language roles and YAML scalar values) map onto at least four
 * roles of the family palette, and the roles that matter for each format are there.
 */
import { Language, LanguageSupport } from '@codemirror/language';
import { highlightTree, tagHighlighter } from '@lezer/highlight';
import { describe, expect, it } from 'vitest';
import { SYNTAX_ROLES, type SyntaxRole } from '@/components/ui/CodeView/theme';
import { SAMPLE_FILES } from '@/dev/mock-samples';
import { LANGUAGE_ROLES, loadLanguage, yamlScalarRole } from './languageSupport';
import { detectLanguage, firstLineOf } from './languages';

function languageOf(extension: unknown): Language | null {
  if (extension instanceof Language) return extension;
  if (extension instanceof LanguageSupport) return extension.language;
  if (Array.isArray(extension)) {
    for (const item of extension) {
      const language = languageOf(item);
      if (language) return language;
    }
  }
  return null;
}

const base = tagHighlighter(SYNTAX_ROLES.map((spec) => ({ tag: spec.tags, class: spec.role })));

/** Role → a few example tokens, as CodeView would colour them. */
async function rolesOf(path: string, text: string) {
  const id = detectLanguage(path, firstLineOf(text));
  const language = languageOf(await loadLanguage(id));
  const roles = new Map<SyntaxRole, Set<string>>();
  const add = (role: SyntaxRole, token: string) => {
    const set = roles.get(role) ?? new Set();
    if (set.size < 5) set.add(token.trim().slice(0, 24));
    roles.set(role, set);
  };
  if (!language) return { id, roles };
  const tree = language.parser.parse(text);
  const extra = LANGUAGE_ROLES[id] ?? [];
  highlightTree(
    tree,
    {
      style: (tags) =>
        extra.find(({ tag }) => tags.includes(tag))?.role ?? base.style(tags) ?? null,
    },
    (from, to, classes) => add(classes.split(' ')[0] as SyntaxRole, text.slice(from, to)),
  );
  if (id === 'yaml') {
    tree.iterate({
      enter: (node) => {
        if (node.name === 'Literal' && node.node.parent?.name !== 'Key') {
          const value = text.slice(node.from, node.to);
          add(yamlScalarRole(value), value);
        }
      },
    });
  }
  return { id, roles };
}

const EXPECTED: Record<string, SyntaxRole[]> = {
  '/var/www/api/index.php': ['keyword', 'variable', 'function', 'string', 'comment', 'type'],
  '/var/www/api/main.go': ['keyword', 'function', 'type', 'string', 'constant', 'property'],
  '/var/www/api/config.yaml': ['property', 'string', 'constant', 'comment'],
  '/var/www/api/composer.json': ['property', 'string', 'punctuation'],
  '/etc/nginx/sites-available/default': ['keyword', 'variable', 'string', 'constant', 'comment'],
  '/var/www/api/Dockerfile': ['keyword', 'property', 'variable', 'string', 'comment'],
  '/var/www/api/deploy.sh': ['keyword', 'variable', 'function', 'string', 'comment'],
  '/var/www/api/.env': ['property', 'string', 'constant', 'comment'],
  '/var/www/api/app.ini': ['tag', 'property', 'string', 'constant', 'comment'],
  '/var/www/api/pyproject.toml': ['tag', 'property', 'string'],
  '/etc/apache2/sites-available/000-default.conf': ['keyword', 'tag', 'string', 'comment'],
};

describe('syntax roles per language', () => {
  for (const [path, text] of Object.entries(SAMPLE_FILES)) {
    // Plain-text samples (notes, ssh config, go.mod) have no grammar on purpose.
    if (path.includes('/legacy/') || /\.txt$|\/config$|go\.mod$/.test(path)) continue;
    it(`${path} is multi-coloured`, async () => {
      const { id, roles } = await rolesOf(path, text);
      if (process.env.SYNTAX_REPORT) {
        console.log(
          `\n${path} (${id})\n` +
            [...roles].map(([role, tokens]) => `  ${role}: ${[...tokens].join(' | ')}`).join('\n'),
        );
      }
      expect(roles.size, `${path} (${id})`).toBeGreaterThanOrEqual(4);
      for (const role of EXPECTED[path] ?? []) expect([...roles.keys()], path).toContain(role);
    });
  }

  it('reads YAML plain scalars as constants or strings', () => {
    for (const value of ['8080', 'true', 'off', 'null', '~', '-1.5', '0x1f', '1e3']) {
      expect(yamlScalarRole(value), value).toBe('constant');
    }
    for (const value of ['0.0.0.0', '15s', 'info', 'mysql', 'v1.2']) {
      expect(yamlScalarRole(value), value).toBe('string');
    }
  });
});
