import { describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';
import postcss from 'postcss';
// eslint-disable-next-line @typescript-eslint/no-var-requires
const tailwindcss = require('tailwindcss');
// eslint-disable-next-line @typescript-eslint/no-var-requires
const resolveConfigPath = require('tailwindcss/lib/util/resolveConfigPath.js').default;

/**
 * layout.tsx now hardcodes `<html className="dark">` with no ThemeProvider,
 * next-themes, or toggle anywhere in the app. Several pre-existing shadcn UI
 * primitives (untouched by this restyle) use `dark:` variants that compile to
 * `:is(.dark *)` selectors - i.e. "apply when any ancestor has class=dark".
 * Before this change, `.dark` was never present anywhere, so those variants
 * were permanently dormant. Now every element in the app is a descendant of
 * `<html class="dark">`, so those dormant styles are unconditionally live -
 * a real behavior change for code this restyle never touched.
 */
describe('hardcoded .dark class activates previously-dormant dark: variants app-wide', () => {
  const frontendRoot = path.resolve(__dirname, '..', '..');

  test('layout.tsx hardcodes the dark class on <html> with no theme toggle anywhere in the app', () => {
    const layoutSrc = fs.readFileSync(path.join(frontendRoot, 'src/app/layout.tsx'), 'utf8');
    expect(layoutSrc).toContain('<html lang="en" className="dark">');

    const hasThemeProvider =
      fs.readdirSync(path.join(frontendRoot, 'src'), { recursive: true } as any)
        .filter((f): f is string => typeof f === 'string' && /\.(tsx|ts)$/.test(f))
        .some(f => {
          const contents = fs.readFileSync(path.join(frontendRoot, 'src', f), 'utf8');
          return /next-themes|ThemeProvider/.test(contents);
        });
    expect(hasThemeProvider).toBe(false);
  });

  test('dark: variants in untouched shadcn primitives (input-group.tsx, alert.tsx) compile to ":is(.dark *)" ancestor selectors', async () => {
    const cwd = process.cwd();
    process.chdir(frontendRoot);
    try {
      const config = require(resolveConfigPath(undefined, { base: frontendRoot }));
      config.content = [
        { raw: '<div class="dark:bg-input/30 dark:border-destructive"></div>', extension: 'html' },
      ];
      const result = await postcss([tailwindcss(config)]).process('@tailwind utilities;', {
        from: path.join(frontendRoot, 'src/app/globals.css'),
      });

      // Because layout.tsx puts .dark on <html>, this ancestor selector now
      // matches every single element in the app - the dark: rules below are
      // permanently "on" instead of permanently "off" as they were pre-restyle.
      expect(result.css).toContain('.dark\\:bg-input\\/30:is(.dark *)');
      expect(result.css).toContain('.dark\\:border-destructive:is(.dark *)');
    } finally {
      process.chdir(cwd);
    }
  });
});
