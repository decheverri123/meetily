import { describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';

/**
 * The meeting note is rendered by @blocknote/shadcn, which does not theme its
 * surfaces off the --bn-colors-* variables globals.css was overriding. It
 * ships a second copy of the shadcn palette scoped to .bn-shadcn and paints
 * the editor from it, so the note rendered as an opaque navy card sitting on
 * the glass panel - a purely visual break that the build, the type checker,
 * and every other test pass straight through.
 *
 * Two things have to hold for the override to actually apply, and neither is
 * visible from the TypeScript side:
 *   1. it lands OUTSIDE @layer, because the vendor stylesheet is unlayered and
 *      unlayered normal declarations beat layered ones regardless of
 *      specificity, and
 *   2. the custom properties carry !important, since ours and the vendor's
 *      selectors are equally specific and CSS file order isn't ours to pick.
 */
describe('@blocknote/shadcn surface override', () => {
  const frontendRoot = path.resolve(__dirname, '..', '..');
  const globalsCss = fs.readFileSync(path.join(frontendRoot, 'src/app/globals.css'), 'utf8');

  test('the vendor stylesheet really does paint the editor from its own --background', () => {
    const vendorCss = fs.readFileSync(
      path.join(frontendRoot, 'node_modules/@blocknote/shadcn/dist/style.css'),
      'utf8'
    );

    // The bug being guarded against, not a hypothetical one.
    expect(vendorCss).toContain('.bn-shadcn .bn-editor{background-color:hsl(var(--background))');
    expect(vendorCss).toMatch(/\.bn-shadcn\.dark\{[^}]*--background: 222\.2 84% 4\.9%/);
  });

  test('the override sits outside every @layer, where it can outrank the unlayered vendor rules', () => {
    const overrideIndex = globalsCss.indexOf('.bn-shadcn,');
    expect(overrideIndex).toBeGreaterThan(-1);

    // Every @layer block in the file closes before the override starts.
    const layerDepthAtOverride = [...globalsCss.slice(0, overrideIndex).matchAll(/@layer[^{]*\{|\{|\}/g)]
      .reduce((depth, match) => depth + (match[0].endsWith('{') ? 1 : -1), 0);
    expect(layerDepthAtOverride).toBe(0);
  });

  test('the palette override and the transparent editor surface are !important', () => {
    expect(globalsCss).toMatch(/\.bn-shadcn,\s*\.bn-shadcn\.dark\s*\{[^}]*--background:\s*var\(--app-card\)\s*!important/);
    expect(globalsCss).toMatch(/\.bn-shadcn \.bn-editor \{\s*background-color:\s*transparent\s*!important/);
  });

  test('BlockNote borders resolve to an opaque token, never the un-suffixed white --border', () => {
    // BlockNote consumes borders as bare hsl(var(--border)) with no alpha
    // slot, so pointing it at the glass --border (pure white, only correct
    // with an opacity suffix) would paint solid white gridlines.
    expect(globalsCss).toMatch(/--border:\s*var\(--app-hairline\)\s*!important/);
    expect(globalsCss).toMatch(/--app-hairline:\s*\d/);
  });
});
