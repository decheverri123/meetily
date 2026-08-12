import { describe, expect, test } from 'bun:test';
import { isLikelyYoutubeUrl, parseQueueInput } from '../../src/hooks/useYoutubeBatchImport';

describe('isLikelyYoutubeUrl', () => {
  test('accepts youtube.com watch URLs', () => {
    expect(isLikelyYoutubeUrl('https://www.youtube.com/watch?v=dQw4w9WgXcQ')).toBe(true);
    expect(isLikelyYoutubeUrl('https://youtube.com/watch?v=abc')).toBe(true);
    expect(isLikelyYoutubeUrl('http://youtube.com/watch?v=abc')).toBe(true);
    expect(isLikelyYoutubeUrl('https://m.youtube.com/watch?v=abc')).toBe(true);
  });

  test('accepts youtu.be short links', () => {
    expect(isLikelyYoutubeUrl('https://youtu.be/dQw4w9WgXcQ')).toBe(true);
    expect(isLikelyYoutubeUrl('https://youtu.be/abc123')).toBe(true);
  });

  test('accepts shorts / embed / live URLs', () => {
    expect(isLikelyYoutubeUrl('https://www.youtube.com/shorts/abc123')).toBe(true);
    expect(isLikelyYoutubeUrl('https://www.youtube.com/embed/abc123')).toBe(true);
    expect(isLikelyYoutubeUrl('https://www.youtube.com/live/abc123')).toBe(true);
  });

  test('rejects non-youtube hosts', () => {
    expect(isLikelyYoutubeUrl('https://vimeo.com/12345')).toBe(false);
    expect(isLikelyYoutubeUrl('https://example.com/watch?v=abc')).toBe(false);
  });

  test('rejects empty / garbage', () => {
    expect(isLikelyYoutubeUrl('')).toBe(false);
    expect(isLikelyYoutubeUrl('not a url')).toBe(false);
    expect(isLikelyYoutubeUrl('youtube.com')).toBe(false);
  });

  test('rejects watch URLs without a v= param', () => {
    expect(isLikelyYoutubeUrl('https://www.youtube.com/watch')).toBe(false);
  });
});

describe('parseQueueInput', () => {
  test('returns empty array for empty input', () => {
    expect(parseQueueInput('')).toEqual([]);
    expect(parseQueueInput('\n\n   \n')).toEqual([]);
  });

  test('splits lines and trims whitespace', () => {
    const result = parseQueueInput('  https://youtu.be/a\n  https://youtu.be/b  \n\n');
    expect(result.length).toBe(2);
    expect(result[0].url).toBe('https://youtu.be/a');
    expect(result[1].url).toBe('https://youtu.be/b');
  });

  test('dedups duplicates preserving first occurrence', () => {
    const result = parseQueueInput('https://youtu.be/a\nhttps://youtu.be/b\nhttps://youtu.be/a\nhttps://youtu.be/c');
    expect(result.length).toBe(3);
    expect(result[0].url).toBe('https://youtu.be/a');
    expect(result[1].url).toBe('https://youtu.be/b');
    expect(result[2].url).toBe('https://youtu.be/c');
  });

  test('marks invalid URLs and includes them in result', () => {
    const result = parseQueueInput('https://youtu.be/a\nnot-a-url\nhttps://example.com/x');
    expect(result.length).toBe(3);
    expect(result[0].valid).toBe(true);
    expect(result[0].error).toBeNull();
    expect(result[1].valid).toBe(false);
    expect(result[1].error).not.toBeNull();
    expect(result[2].valid).toBe(false);
  });

  test('initializes title as empty string', () => {
    const result = parseQueueInput('https://youtu.be/a');
    expect(result[0].title).toBe('');
  });

  test('handles CRLF line endings', () => {
    const result = parseQueueInput('https://youtu.be/a\r\nhttps://youtu.be/b\r\n');
    expect(result.length).toBe(2);
    expect(result[0].url).toBe('https://youtu.be/a');
    expect(result[1].url).toBe('https://youtu.be/b');
  });
});
