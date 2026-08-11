import { describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';

/**
 * The glass re-theme redefines --border/--input/--secondary/--muted/--ring as
 * pure white (`0 0% 100%`) HSL triplets, on the assumption that every
 * consumer applies an opacity suffix (e.g. `border-border/10`). A consumer
 * that uses one of these tokens WITHOUT an opacity suffix renders solid
 * opaque white instead of the intended subtle translucent line - a glaring
 * visual bug against the dark glass background.
 *
 * components/ui/scroll-area.tsx is a real, currently-used shadcn primitive
 * (ModelSettingsModal, TranscriptRecovery) that was NOT touched by the
 * restyle diff, so its `bg-border` scrollbar-thumb class never got an
 * opacity suffix added when --border was repointed to opaque white.
 */
describe('glass color tokens used without an opacity suffix', () => {
  const frontendRoot = path.resolve(__dirname, '..', '..');

  test('--border resolves to pure opaque white (0 0% 100%), so any un-suffixed consumer is a real bug, not a theoretical one', () => {
    const css = fs.readFileSync(path.join(frontendRoot, 'src/app/globals.css'), 'utf8');
    const match = css.match(/--border:\s*([^;]+);/);
    expect(match?.[1].trim()).toBe('0 0% 100%');
  });

  test('components/ui/scroll-area.tsx scrollbar thumb should use an opacity-suffixed border token, not solid bg-border', () => {
    const scrollAreaSrc = fs.readFileSync(
      path.join(frontendRoot, 'src/components/ui/scroll-area.tsx'),
      'utf8'
    );
    const thumbClassMatch = scrollAreaSrc.match(/ScrollAreaThumb className="([^"]+)"/);
    expect(thumbClassMatch).not.toBeNull();
    const thumbClasses = thumbClassMatch![1];

    // Every other consumer of these tokens in the restyled files uses an
    // opacity suffix (e.g. "border-border/10", "bg-secondary/5"). This
    // pre-existing, still-live component was missed, so it renders a solid
    // opaque white scrollbar thumb against the dark glass UI.
    expect(thumbClasses).toMatch(/bg-border\/\d/);
  });

  test('ScrollArea (with the un-suffixed thumb) is actually still rendered elsewhere in the app, not dead code', () => {
    const consumers = [
      'src/components/ModelSettingsModal.tsx',
      'src/components/TranscriptRecovery/TranscriptRecovery.tsx',
    ];
    for (const consumer of consumers) {
      const consumerSrc = fs.readFileSync(path.join(frontendRoot, consumer), 'utf8');
      expect(consumerSrc).toContain('ScrollArea');
    }
  });
});
