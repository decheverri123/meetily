/**
 * Storage Service
 *
 * Handles all meeting storage and retrieval Tauri backend calls (SQLite persistence).
 * Pure 1-to-1 wrapper - no error handling changes, exact same behavior as direct invoke calls.
 */

import { invoke } from '@tauri-apps/api/core';
import { Transcript } from '@/types';

export interface SaveMeetingRequest {
  meetingTitle: string;
  transcripts: Transcript[];
  folderPath: string | null;
}

export interface SaveMeetingResponse {
  meeting_id: string;
}

export interface Meeting {
  id: string;
  title: string;
  icon?: string;
  [key: string]: any; // Allow additional properties from backend
}

export interface Folder {
  id: string;
  name: string;
  icon?: string;
  is_auto: boolean;
  created_at: string;
  updated_at: string;
}

export interface MeetingWithFolder {
  id: string;
  title: string;
  icon?: string;
  folder_id: string | null;
  folder_name: string | null;
}

export interface CategorizeResult {
  meeting_id: string;
  folder_id: string | null;
  folder_name: string | null;
  suggested_new_folder: string | null;
}

export interface BatchCategorizeResult {
  total: number;
  assigned: number;
  suggested_new: number;
  failed: number;
  results: CategorizeResult[];
}

/**
 * Storage Service
 * Singleton service for managing meeting storage operations
 */
export class StorageService {
  /**
   * Save meeting transcript to SQLite database
   * @param meetingTitle - Title of the meeting
   * @param transcripts - Array of transcript segments
   * @param folderPath - Optional folder path for audio file
   * @returns Promise with { meeting_id: string }
   */
  async saveMeeting(
    meetingTitle: string,
    transcripts: Transcript[],
    folderPath: string | null
  ): Promise<SaveMeetingResponse> {
    return invoke<SaveMeetingResponse>('api_save_transcript', {
      meetingTitle,
      transcripts,
      folderPath,
    });
  }

  /**
   * Get meeting details by ID
   * @param meetingId - ID of the meeting to fetch
   * @returns Promise with meeting details
   */
  async getMeeting(meetingId: string): Promise<Meeting> {
    return invoke<Meeting>('api_get_meeting', { meetingId });
  }

  /**
   * Get list of all meetings
   * @returns Promise with array of meetings
   */
  async getMeetings(): Promise<Meeting[]> {
    return invoke<Meeting[]>('api_get_meetings');
  }

  async getFolders(): Promise<Folder[]> {
    return invoke<Folder[]>('api_get_folders');
  }

  async createFolder(name: string): Promise<Folder> {
    return invoke<Folder>('api_create_folder', { name });
  }

  async renameFolder(id: string, name: string): Promise<boolean> {
    return invoke<boolean>('api_rename_folder', { id, name });
  }

  async deleteFolder(id: string): Promise<boolean> {
    return invoke<boolean>('api_delete_folder', { id });
  }

  async updateFolderIcon(id: string, icon: string): Promise<boolean> {
    return invoke<boolean>('api_update_folder_icon', { id, icon });
  }

  async updateMeetingIcon(id: string, icon: string): Promise<boolean> {
    return invoke<boolean>('api_update_meeting_icon', { id, icon });
  }

  async assignMeetingToFolder(
    meetingId: string,
    folderId: string | null
  ): Promise<boolean> {
    return invoke<boolean>('api_assign_meeting_to_folder', {
      meetingId,
      folderId,
    });
  }

  async aiCategorizeMeeting(
    meetingId: string,
    folderIds?: string[]
  ): Promise<CategorizeResult> {
    return invoke<CategorizeResult>('api_ai_categorize_meeting', {
      meetingId,
      folderIds: folderIds ?? null,
    });
  }

  async aiCategorizeAllMeetings(): Promise<BatchCategorizeResult> {
    return invoke<BatchCategorizeResult>('api_ai_categorize_all_meetings');
  }

  async getMeetingsWithFolders(): Promise<MeetingWithFolder[]> {
    return invoke<MeetingWithFolder[]>('api_get_meetings_with_folders');
  }
}

// Export singleton instance
export const storageService = new StorageService();
