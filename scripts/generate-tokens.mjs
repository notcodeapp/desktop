import { readFile, writeFile, mkdir } from 'node:fs/promises';
const root = new URL('../', import.meta.url);
const tokens = JSON.parse(await readFile(new URL('design-system/tokens.json', root), 'utf8'));
// `version` is metadata and `light` is a theme override, not a base token group.
const groups = Object.entries(tokens).filter(([k, v]) => typeof v === 'object' && k !== 'light');
const lines = groups.flatMap(([group, values]) => Object.entries(values).map(([key, value]) => `  --${group}-${key}: ${value};`));
const lightLines = Object.entries(tokens.light ?? {}).flatMap(([group, values]) => Object.entries(values).map(([key, value]) => `  --${group}-${key}: ${value};`));
const lightBlock = lightLines.length ? `\n:root[data-theme='light'] {\n  color-scheme: light;\n${lightLines.join('\n')}\n}\n` : '';
const css = `/* Generated from design-system/tokens.json. Run npm run tokens. */\n:root {\n  color-scheme: dark;\n${lines.join('\n')}\n}\n${lightBlock}`;
const out = new URL('src/styles/tokens.css', root);
if (process.argv.includes('--check')) {
  if (await readFile(out, 'utf8') !== css) throw new Error('Design tokens are out of sync');
} else {
  await mkdir(new URL('src/styles/', root), { recursive: true });
  await writeFile(out, css);
}
