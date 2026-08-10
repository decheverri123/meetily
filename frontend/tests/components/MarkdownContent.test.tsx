import { describe, expect, test } from 'bun:test';
import { render } from '@testing-library/react';
import { MarkdownContent } from '../../src/components/MarkdownContent';

/**
 * Adversarial LLM output rendered through `MarkdownContent` (react-markdown +
 * remark-gfm, no rehype-raw). react-markdown does not parse raw HTML in the
 * source into real DOM elements by default (`allowDangerousHtml` is off
 * unless a rehype plugin like rehype-raw is added, and MarkdownContent.tsx
 * only adds remark-gfm) - so raw `<script>`/`<img onerror>` tags should be
 * rendered as inert text, not executable/attribute-bearing DOM nodes.
 */
describe('MarkdownContent XSS safety', () => {
  test('raw <script> tags in LLM output are not rendered as executable script elements', () => {
    const malicious = 'Recap: <script>window.__PWNED__ = true;</script> meeting notes';
    const { container } = render(<MarkdownContent>{malicious}</MarkdownContent>);

    expect(container.querySelectorAll('script').length).toBe(0);
    expect((window as unknown as { __PWNED__?: boolean }).__PWNED__).toBeUndefined();
  });

  test('an <img onerror=...> payload does not produce a live onerror-bearing <img> element', () => {
    const malicious = '<img src=x onerror="window.__PWNED__=true">';
    const { container } = render(<MarkdownContent>{malicious}</MarkdownContent>);

    const img = container.querySelector('img[onerror]');
    expect(img).toBeNull();
  });

  test('a huge single-line LLM output (no whitespace to wrap on) renders without throwing', () => {
    // 500k chars, one "word" - worst case for markdown parsing / layout.
    const huge = 'a'.repeat(500_000);
    expect(() => render(<MarkdownContent>{huge}</MarkdownContent>)).not.toThrow();
  });

  test('deeply nested/malformed markdown (unterminated constructs) renders without throwing', () => {
    const malformed =
      '['.repeat(2000) +
      'text' +
      ']('.repeat(2000) +
      '***unterminated bold and italic markers everywhere' +
      '`'.repeat(500);
    expect(() => render(<MarkdownContent>{malformed}</MarkdownContent>)).not.toThrow();
  });
});
