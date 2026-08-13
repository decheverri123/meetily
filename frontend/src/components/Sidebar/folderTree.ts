import type { Folder, MeetingWithFolder } from '@/services/storageService';

export interface SidebarItem {
  id: string;
  title: string;
  type: 'folder' | 'file';
  icon?: string;
  isAuto?: boolean;
  children?: SidebarItem[];
  meetingId?: string;
  folderId?: string;
}

export const UNFILED_FOLDER_ID = 'unfiled';
export const FOLDER_ROOT_ID = 'folders-root';

export function buildSidebarTree(
  folders: Folder[],
  meetings: MeetingWithFolder[]
): SidebarItem[] {
  // Normalize at the API boundary: some backends return `folder_id: ""`
  // instead of `null` for unfiled meetings. `"" == null` is false, so a
  // meeting with an empty string wouldn't be classified as unfiled and
  // would also not match any folder — silently dropping it from the tree.
  const normalizedMeetings = meetings.map((m) => ({
    ...m,
    folder_id: m.folder_id === "" ? null : m.folder_id,
  }));

  const folderItems: SidebarItem[] = folders
    .slice()
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }))
    .map((f) => {
      const children = normalizedMeetings
        .filter((m) => m.folder_id === f.id)
        .map((m) => ({
          id: `m:${m.id}`,
          title: m.title,
          type: 'file' as const,
          icon: m.icon,
          meetingId: m.id,
          folderId: f.id,
        }));
      return {
        id: `f:${f.id}`,
        title: f.name,
        type: 'folder' as const,
        icon: f.icon,
        isAuto: f.is_auto,
        children: children,
        folderId: f.id,
      };
    });

  const unfiledMeetings = normalizedMeetings.filter((m) => m.folder_id == null);
  const unfiledChildren: SidebarItem[] = unfiledMeetings.map((m) => ({
    id: `m:${m.id}`,
    title: m.title,
    type: 'file' as const,
    icon: m.icon,
    meetingId: m.id,
  }));

  if (unfiledChildren.length > 0) {
    folderItems.push({
      id: UNFILED_FOLDER_ID,
      title: 'Unfiled',
      type: 'folder',
      children: unfiledChildren,
    });
  }

  return [
    {
      id: FOLDER_ROOT_ID,
      title: 'Meeting Notes',
      type: 'folder',
      children: folderItems,
    },
  ];
}

export function findItemById(
  items: SidebarItem[],
  id: string
): SidebarItem | undefined {
  for (const item of items) {
    if (item.id === id) return item;
    if (item.children) {
      const found = findItemById(item.children, id);
      if (found) return found;
    }
  }
  return undefined;
}
