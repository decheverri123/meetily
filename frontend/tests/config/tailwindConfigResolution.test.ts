import { describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';
import postcss from 'postcss';
// eslint-disable-next-line @typescript-eslint/no-var-requires
const tailwindcss = require('tailwindcss');
// eslint-disable-next-line @typescript-eslint/no-var-requires
const resolveConfigPath = require('tailwindcss/lib/util/resolveConfigPath.js').default;

/**
 * The glass re-theme originally added its new HSL tokens/fonts/colors to
 * frontend/tailwind.config.ts, but frontend/tailwind.config.js (the
 * pre-existing, more complete shadcn-generated config - darkMode, accordion
 * keyframes for the Radix Accordion primitive, tertiary color) was still
 * present too, and postcss.config.js/.mjs invoke `tailwindcss: {}` with no
 * explicit path. Per tailwindcss v3's resolveConfigPath() priority, ".js" is
 * checked before ".ts", so the .ts re-theme was silently dead in the real
 * build. Fix: port the re-theme into the actually-resolved tailwind.config.js
 * and delete tailwind.config.ts, so there's exactly one config file and no
 * silent-shadowing trap for the next person.
 */
describe('tailwind config resolution (glass re-theme)', () => {
  const frontendRoot = path.resolve(__dirname, '..', '..');

  test('tailwind.config.ts no longer exists - tailwind.config.js is the single source of truth', () => {
    expect(fs.existsSync(path.join(frontendRoot, 'tailwind.config.ts'))).toBe(false);
    expect(fs.existsSync(path.join(frontendRoot, 'tailwind.config.js'))).toBe(true);
  });

  test('the config actually resolved by the real postcss pipeline is tailwind.config.js', () => {
    const resolved = resolveConfigPath(undefined, { base: frontendRoot });
    expect(resolved).toBe(path.join(frontendRoot, 'tailwind.config.js'));
  });

  test('font-sans compiles to the new --font-instrument-sans variable (not the removed --font-source-sans-3)', async () => {
    const cwd = process.cwd();
    process.chdir(frontendRoot);
    try {
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const config = require(resolveConfigPath(undefined, { base: frontendRoot }));
      config.content = [{ raw: '<div class="font-sans"></div>', extension: 'html' }];

      const css = '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n';
      const result = await postcss([tailwindcss(config)]).process(css, {
        from: path.join(frontendRoot, 'src/app/globals.css'),
      });

      expect(result.css).toContain('var(--font-instrument-sans)');
      expect(result.css).not.toContain('var(--font-source-sans-3)');
    } finally {
      process.chdir(cwd);
    }
  });

  test('the new accent-violet and success color tokens actually compile to real utility classes', async () => {
    const cwd = process.cwd();
    process.chdir(frontendRoot);
    try {
      const config = require(resolveConfigPath(undefined, { base: frontendRoot }));
      config.content = [
        { raw: '<div class="text-accent-violet bg-accent-violet/10 text-success bg-success/10"></div>', extension: 'html' },
      ];

      const css = '@tailwind base;\n@tailwind components;\n@tailwind utilities;\n';
      const result = await postcss([tailwindcss(config)]).process(css, {
        from: path.join(frontendRoot, 'src/app/globals.css'),
      });

      expect(result.css).toContain('.text-accent-violet');
      expect(result.css).toContain('.text-success');
    } finally {
      process.chdir(cwd);
    }
  });
});
