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

const folders: Folder[] = [
  { id: 'f1', name: 'Q4 Planning', is_auto: false, created_at: '', updated_at: '' },
  { id: 'f2', name: 'Customer Calls', is_auto: false, created_at: '', updated_at: '' },
];

const meetings: MeetingWithFolder[] = [
  { id: 'm1', title: 'Standup', folder_id: 'f1', folder_name: 'Q4 Planning' },
  { id: 'm2', title: 'Roadmap', folder_id: 'f1', folder_name: 'Q4 Planning' },
  { id: 'm3', title: 'Acme call', folder_id: 'f2', folder_name: 'Customer Calls' },
  { id: 'm4', title: 'Loose notes', folder_id: null, folder_name: null },
];

describe('buildSidebarTree', () => {
  test('wraps everything in a Meeting Notes root folder', () => {
    const tree = buildSidebarTree(folders, meetings);
    expect(tree).toHaveLength(1);
    expect(tree[0].id).toBe(FOLDER_ROOT_ID);
    expect(tree[0].type).toBe('folder');
    expect(tree[0].title).toBe('Meeting Notes');
  });

  test('sorts folders case-insensitively by name', () => {
    const messy: Folder[] = [
      { id: 'b', name: 'Bravo', is_auto: false, created_at: '', updated_at: '' },
      { id: 'a', name: 'alpha', is_auto: false, created_at: '', updated_at: '' },
    ];
    const tree = buildSidebarTree(messy, []);
    const childIds = tree[0].children!.map((c) => c.id);
    expect(childIds).toEqual(['f:a', 'f:b']);
  });

  test('attaches each meeting to its folder', () => {
    const tree = buildSidebarTree(folders, meetings);
    const q4 = tree[0].children!.find((c) => c.id === 'f:f1')!;
    expect(q4.children!.map((c) => c.id)).toEqual(['m:m1', 'm:m2']);
    const cust = tree[0].children!.find((c) => c.id === 'f:f2')!;
    expect(cust.children!.map((c) => c.id)).toEqual(['m:m3']);
  });

  test('collects meetings with no folder under a single Unfiled group', () => {
    const tree = buildSidebarTree(folders, meetings);
    const unfiled = tree[0].children!.find((c) => c.id === UNFILED_FOLDER_ID)!;
    expect(unfiled).toBeTruthy();
    expect(unfiled.children!.map((c) => c.id)).toEqual(['m:m4']);
  });

  test('omits the Unfiled group when nothing is unfiled', () => {
    const allFiled: MeetingWithFolder[] = meetings.map((m) => ({
      ...m,
      folder_id: 'f1',
      folder_name: 'Q4 Planning',
    }));
    const tree = buildSidebarTree(folders, allFiled);
    expect(tree[0].children!.some((c) => c.id === UNFILED_FOLDER_ID)).toBe(false);
  });

  test('emits empty folder children when no meetings belong to a folder', () => {
    const tree = buildSidebarTree(folders, []);
    const childIds = tree[0].children!.map((c) => c.id);
    expect(childIds.sort()).toEqual(['f:f1', 'f:f2']);
    for (const c of tree[0].children!) {
      expect(c.children).toEqual([]);
    }
  });
});

describe('findItemById', () => {
  test('locates a top-level item', () => {
    const tree = buildSidebarTree(folders, meetings);
    expect(findItemById(tree, FOLDER_ROOT_ID)?.title).toBe('Meeting Notes');
  });

  test('locates a nested file', () => {
    const tree = buildSidebarTree(folders, meetings);
    expect(findItemById(tree, 'm:m1')?.title).toBe('Standup');
  });

  test('returns undefined for missing ids', () => {
    const tree = buildSidebarTree(folders, meetings);
    expect(findItemById(tree, 'nope')).toBeUndefined();
  });
});
