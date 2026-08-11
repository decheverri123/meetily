import { describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';

/**
 * --radius is a single global CSS custom property (not scoped to the
 * restyled live-recording files), so bumping it would resize
 * rounded-lg/rounded-md/rounded-sm on every unrelated small control app-wide
 * (e.g. the shared shadcn Button's `sm`/`default` variants) that this
 * restyle never reviewed. The new glass-* surface classes (.glass-panel,
 * .glass-pill, etc.) all use explicit arbitrary radii (rounded-[20px],
 * rounded-2xl, rounded-full) rather than --radius, so there was never a
 * reason to change the global value. Fix: leave --radius at its original
 * 0.5rem.
 */
describe('--radius stays untouched so unreviewed UI outside the 19 changed files is unaffected', () => {
  const frontendRoot = path.resolve(__dirname, '..', '..');

  test('--radius remains 0.5rem (8px), unchanged by the glass re-theme', () => {
    const css = fs.readFileSync(path.join(frontendRoot, 'src/app/globals.css'), 'utf8');
    const match = css.match(/--radius:\s*([^;]+);/);
    expect(match?.[1].trim()).toBe('0.5rem');
  });

  test("Button's small variant (h-8 = 32px tall, untouched by the restyle) keeps its original, subtle corner radius", () => {
    const buttonSrc = fs.readFileSync(path.join(frontendRoot, 'src/components/ui/button.tsx'), 'utf8');
    expect(buttonSrc).toContain('sm: "h-8 rounded-md px-3 text-xs"');

    // rounded-md = calc(var(--radius) - 4px) in tailwind.config.js.
    const radiusPx = 8; // 0.5rem
    const mdRadiusPx = radiusPx - 4; // 4px
    const buttonHeightPx = 32; // h-8

    // A corner radius that is >25% of the control's height starts reading as
    // "pill-shaped" rather than "rounded rectangle" across Settings, dialogs,
    // and other screens this restyle never touched.
    const radiusFractionOfHeight = mdRadiusPx / buttonHeightPx;
    expect(radiusFractionOfHeight).toBeLessThan(0.25);
  });
});
