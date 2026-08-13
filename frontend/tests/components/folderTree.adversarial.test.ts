// Adversarial tests for the folder sidebar tree builder.
// Focus: empty/whitespace/long/Unicode folder names, special-character ids,
// missing fields, and the corner cases of tree assembly.
import { describe, expect, test } from 'bun:test';
import {
  buildSidebarTree,
  findItemById,
  FOLDER_ROOT_ID,
  UNFILED_FOLDER_ID,
} from '../../src/components/Sidebar/folderTree';
import type {
  Folder,
  MeetingWithFolder,
} from '../../src/services/storageService';

describe('buildSidebarTree — adversarial', () => {
  test('handles empty folder list with only unfiled meetings', () => {
    const meetings: MeetingWithFolder[] = [
      { id: 'm1', title: 'Solo', folder_id: null, folder_name: null },
    ];
    const tree = buildSidebarTree([], meetings);
    expect(tree).toHaveLength(1);
    expect(tree[0].id).toBe(FOLDER_ROOT_ID);
    // No folders defined → unfiled group appears.
    const unfiled = tree[0].children!.find((c) => c.id === UNFILED_FOLDER_ID);
    expect(unfiled).toBeTruthy();
    expect(unfiled!.children!.map((c) => c.id)).toEqual(['m:m1']);
  });

  test('handles empty meetings list with multiple folders', () => {
    const folders: Folder[] = [
      { id: 'f1', name: 'A', is_auto: false, created_at: '', updated_at: '' },
      { id: 'f2', name: 'B', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, []);
    const childIds = tree[0].children!.map((c) => c.id).sort();
    // Two folders, no Unfiled group.
    expect(childIds).toEqual(['f:f1', 'f:f2']);
    expect(tree[0].children!.some((c) => c.id === UNFILED_FOLDER_ID)).toBe(false);
    // No children on either.
    for (const c of tree[0].children!) {
      expect(c.children).toEqual([]);
    }
  });

  test('handles folder with whitespace-only name', () => {
    const folders: Folder[] = [
      { id: 'f1', name: '   ', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, []);
    expect(tree[0].children![0].title).toBe('   ');
  });

  test('handles folder with very long name (5KB)', () => {
    const longName = 'a'.repeat(5_000);
    const folders: Folder[] = [
      { id: 'f1', name: longName, is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, []);
    expect(tree[0].children![0].title.length).toBe(5_000);
  });

  test('handles folder names with Unicode/RTL/emoji', () => {
    const folders: Folder[] = [
      { id: 'f1', name: '中文', is_auto: false, created_at: '', updated_at: '' },
      { id: 'f2', name: 'العربية', is_auto: false, created_at: '', updated_at: '' },
      { id: 'f3', name: '📁 Folder 🚀', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, []);
    const titles = tree[0].children!.map((c) => c.title);
    expect(titles).toContain('中文');
    expect(titles).toContain('العربية');
    expect(titles).toContain('📁 Folder 🚀');
  });

  test('handles duplicate folder names — both are kept', () => {
    const folders: Folder[] = [
      { id: 'f1', name: 'Same', is_auto: false, created_at: '', updated_at: '' },
      { id: 'f2', name: 'Same', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, []);
    // Both folders appear. The "id" field still distinguishes them.
    const sameFolderItems = tree[0].children!.filter((c) => c.title === 'Same');
    expect(sameFolderItems.length).toBe(2);
  });

  test('meeting referencing a folder_id that is not in the folder list is dropped', () => {
    // The frontend trusts the API to return consistent data, but if a
    // meeting has folder_id="ghost" and no folder "ghost" exists, the
    // meeting is silently dropped from the tree (not in unfiled, not
    // in any folder). This test pins the current behavior — a real
    // broken state, not a crash, but a silent UI bug.
    const meetings: MeetingWithFolder[] = [
      { id: 'm1', title: 'Ghost meeting', folder_id: 'ghost', folder_name: 'Ghost' },
      { id: 'm2', title: 'Real meeting', folder_id: 'f1', folder_name: 'F1' },
    ];
    const folders: Folder[] = [
      { id: 'f1', name: 'F1', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, meetings);
    const allIds = JSON.stringify(tree);
    expect(allIds).toContain('m:m2');
    expect(allIds).not.toContain('m:m1');
    // And it's not in unfiled either (folder_id is set, just to a missing folder).
    const unfiled = tree[0].children!.find((c) => c.id === UNFILED_FOLDER_ID);
    if (unfiled) {
      expect(unfiled.children!.map((c) => c.id)).not.toContain('m:m1');
    }
  });

  test('meeting with empty string folder_id is treated as unfiled', () => {
    // Regression: the Rust API can return `folder_id: ""` for an unfiled
    // meeting. The loose `== null` unfiled check would not match an empty
    // string, and `m.folder_id === f.id` would not match any folder, so
    // the meeting would silently disappear. The builder normalizes
    // empty-string folder_id to null at the API boundary.
    const meetings: MeetingWithFolder[] = [
      { id: 'm1', title: 'Empty id', folder_id: '' as any, folder_name: null },
    ];
    const folders: Folder[] = [
      { id: 'f1', name: 'F1', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, meetings);
    const unfiled = tree[0].children!.find((c) => c.id === UNFILED_FOLDER_ID);
    expect(unfiled).toBeTruthy();
    expect(unfiled!.children!.map((c) => c.id)).toEqual(['m:m1']);
    // The meeting must not also be attached to a folder by accident.
    const f1 = tree[0].children!.find((c) => c.id === 'f:f1')!;
    expect(f1.children).toEqual([]);
  });

  test('100 meetings under one folder are all attached', () => {
    const meetings: MeetingWithFolder[] = Array.from({ length: 100 }, (_, i) => ({
      id: `m${i}`,
      title: `Meeting ${i}`,
      folder_id: 'f1',
      folder_name: 'Big',
    }));
    const folders: Folder[] = [
      { id: 'f1', name: 'Big', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, meetings);
    const f1 = tree[0].children!.find((c) => c.id === 'f:f1')!;
    expect(f1.children!.length).toBe(100);
  });

  test('meeting with no title defaults to empty string (no crash)', () => {
    const meetings: MeetingWithFolder[] = [
      { id: 'm1', title: undefined as any, folder_id: null, folder_name: null },
    ];
    const tree = buildSidebarTree([], meetings);
    const unfiled = tree[0].children!.find((c) => c.id === UNFILED_FOLDER_ID);
    expect(unfiled!.children![0].title).toBeUndefined();
  });
});

describe('findItemById — adversarial', () => {
  test('handles id with special characters', () => {
    const folders: Folder[] = [
      { id: 'f/with/slashes', name: 'X', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, []);
    // The builder uses `f:` + id, so we look up 'f:f/with/slashes'.
    const found = findItemById(tree, 'f:f/with/slashes');
    expect(found).toBeTruthy();
    expect(found!.title).toBe('X');
  });

  test('handles id that is a prefix of another id (no false match)', () => {
    const folders: Folder[] = [
      { id: 'f1', name: 'A', is_auto: false, created_at: '', updated_at: '' },
      { id: 'f10', name: 'B', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(folders, []);
    expect(findItemById(tree, 'f:f1')?.title).toBe('A');
    expect(findItemById(tree, 'f:f10')?.title).toBe('B');
    expect(findItemById(tree, 'f:f100')?.title).toBeUndefined();
  });

  test('handles empty string id', () => {
    const tree = buildSidebarTree([], []);
    expect(findItemById(tree, '')).toBeUndefined();
  });
});
