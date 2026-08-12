'use client';

import React, { createContext, useContext, useState, useEffect } from 'react';
import { usePathname, useRouter } from 'next/navigation';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import {
  storageService,
  type BatchCategorizeResult,
  type Folder,
  type MeetingWithFolder,
} from '@/services/storageService';
import {
  buildSidebarTree,
  type SidebarItem,
} from '@/components/Sidebar/folderTree';

export interface CurrentMeeting {
  id: string;
  title: string;
}

interface TranscriptSearchResult {
  id: string;
  title: string;
  matchContext: string;
  timestamp: string;
};

export interface SidebarContextType {
  currentMeeting: CurrentMeeting | null;
  setCurrentMeeting: (meeting: CurrentMeeting | null) => void;
  sidebarItems: SidebarItem[];
  isCollapsed: boolean;
  toggleCollapse: () => void;
  meetings: MeetingWithFolder[];
  setMeetings: (meetings: MeetingWithFolder[]) => void;
  isMeetingActive: boolean;
  setIsMeetingActive: (active: boolean) => void;
  handleRecordingToggle: () => void;
  searchTranscripts: (query: string) => Promise<void>;
  searchResults: TranscriptSearchResult[];
  isSearching: boolean;
  setServerAddress: (address: string) => void;
  serverAddress: string;
  transcriptServerAddress: string;
  setTranscriptServerAddress: (address: string) => void;
  activeSummaryPolls: Map<string, NodeJS.Timeout>;
  startSummaryPolling: (meetingId: string, processId: string, onUpdate: (result: any) => void) => void;
  stopSummaryPolling: (meetingId: string) => void;
  refetchMeetings: () => Promise<void>;
  folders: Folder[];
  refetchFolders: () => Promise<void>;
  createFolder: (name: string) => Promise<Folder>;
  renameFolder: (id: string, name: string) => Promise<boolean>;
  deleteFolder: (id: string) => Promise<boolean>;
  assignMeetingToFolder: (meetingId: string, folderId: string | null) => Promise<boolean>;
  aiCategorizeMeeting: (meetingId: string, folderIds?: string[]) => Promise<void>;
  aiCategorizeAllMeetings: () => Promise<BatchCategorizeResult>;
}

const SidebarContext = createContext<SidebarContextType | null>(null);

export const useSidebar = () => {
  const context = useContext(SidebarContext);
  if (!context) {
    throw new Error('useSidebar must be used within a SidebarProvider');
  }
  return context;
};

export function SidebarProvider({ children }: { children: React.ReactNode }) {
  const [currentMeeting, setCurrentMeeting] = useState<CurrentMeeting | null>({ id: 'intro-call', title: '+ New Call' });
  const [storedIsCollapsed, setStoredIsCollapsed] = useState(true);
  const [isMeetingDetailsRailExpanded, setIsMeetingDetailsRailExpanded] = useState(false);
  const [meetings, setMeetings] = useState<MeetingWithFolder[]>([]);
  const [sidebarItems, setSidebarItems] = useState<SidebarItem[]>([]);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [isMeetingActive, setIsMeetingActive] = useState(false);
  const [searchResults, setSearchResults] = useState<any[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  const [serverAddress, setServerAddress] = useState('');
  const [transcriptServerAddress, setTranscriptServerAddress] = useState('');
  const [activeSummaryPolls, setActiveSummaryPolls] = useState<Map<string, NodeJS.Timeout>>(new Map());

  const { isRecording } = useRecordingState();

  const pathname = usePathname();
  const router = useRouter();

  const isMeetingDetailsRoute = pathname?.includes('/meeting-details') ?? false;
  const isCollapsed = isMeetingDetailsRoute ? !isMeetingDetailsRailExpanded : storedIsCollapsed;

  useEffect(() => {
    if (!isMeetingDetailsRoute) setIsMeetingDetailsRailExpanded(false);
  }, [isMeetingDetailsRoute]);

  const fetchFolders = React.useCallback(async () => {
    try {
      const list = await storageService.getFolders();
      setFolders(list);
    } catch (error) {
      console.error('Error fetching folders:', error);
      setFolders([]);
    }
  }, []);

  const fetchMeetings = React.useCallback(async () => {
    if (serverAddress) {
      try {
        const meetings = await invoke('api_get_meetings') as Array<{ id: string, title: string }>;
        const transformedMeetings: MeetingWithFolder[] = meetings.map((meeting: any) => ({
          id: meeting.id,
          title: meeting.title,
          folder_id: null,
          folder_name: null,
        }));
        setMeetings(transformedMeetings);
        Analytics.trackBackendConnection(true);
      } catch (error) {
        console.error('Error fetching meetings:', error);
        setMeetings([]);
        Analytics.trackBackendConnection(false, error instanceof Error ? error.message : 'Unknown error');
      }
    }
  }, [serverAddress]);

  const fetchMeetingsWithFolders = React.useCallback(async () => {
    try {
      const list: MeetingWithFolder[] = await storageService.getMeetingsWithFolders();
      setMeetings(list);
      Analytics.trackBackendConnection(true);
    } catch (error) {
      console.error('Error fetching meetings with folders:', error);
      setMeetings([]);
      Analytics.trackBackendConnection(false, error instanceof Error ? error.message : 'Unknown error');
    }
  }, []);

  const refetchAll = React.useCallback(async () => {
    await Promise.all([fetchMeetingsWithFolders(), fetchFolders()]);
  }, [fetchMeetingsWithFolders, fetchFolders]);

  useEffect(() => {
    refetchAll();
  }, [refetchAll, serverAddress]);

  useEffect(() => {
    const fetchSettings = async () => {
      setServerAddress('http://localhost:5167');
      setTranscriptServerAddress('http://127.0.0.1:8178/stream');
    };
    fetchSettings();
  }, []);

  useEffect(() => {
    const tree = buildSidebarTree(folders, meetings);
    setSidebarItems(tree);
  }, [folders, meetings]);

  const toggleCollapse = () => {
    if (isMeetingDetailsRoute) {
      setIsMeetingDetailsRailExpanded(prev => !prev);
    } else {
      setStoredIsCollapsed(prev => !prev);
    }
  };

  useEffect(() => {
    if (pathname === '/') {
      setCurrentMeeting({ id: 'intro-call', title: '+ New Call' });
    }
  }, [pathname]);

  const createFolder = React.useCallback(async (name: string) => {
    const folder = await storageService.createFolder(name);
    await fetchFolders();
    return folder;
  }, [fetchFolders]);

  const renameFolder = React.useCallback(async (id: string, name: string) => {
    const ok = await storageService.renameFolder(id, name);
    await fetchFolders();
    return ok;
  }, [fetchFolders]);

  const deleteFolder = React.useCallback(async (id: string) => {
    const ok = await storageService.deleteFolder(id);
    await Promise.all([fetchFolders(), fetchMeetingsWithFolders()]);
    return ok;
  }, [fetchFolders, fetchMeetingsWithFolders]);

  const assignMeetingToFolder = React.useCallback(async (meetingId: string, folderId: string | null) => {
    const ok = await storageService.assignMeetingToFolder(meetingId, folderId);
    await fetchMeetingsWithFolders();
    return ok;
  }, [fetchMeetingsWithFolders]);

  const aiCategorizeMeeting = React.useCallback(async (meetingId: string, folderIds?: string[]) => {
    await storageService.aiCategorizeMeeting(meetingId, folderIds);
    await refetchAll();
  }, [refetchAll]);

  const aiCategorizeAllMeetings = React.useCallback(async () => {
    const result = await storageService.aiCategorizeAllMeetings();
    await refetchAll();
    return result;
  }, [refetchAll]);

  const handleRecordingToggle = () => {
    if (!isRecording) {
      if (pathname === '/') {
        console.log('Triggering recording from sidebar (already on home page)');
        window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
      } else {
        console.log('Navigating to home page with auto-start flag');
        sessionStorage.setItem('autoStartRecording', 'true');
        router.push('/');
      }

      Analytics.trackButtonClick('start_recording', 'sidebar');
    }
  };

  const searchTranscripts = async (query: string) => {
    if (!query.trim()) {
      setSearchResults([]);
      return;
    }

    try {
      setIsSearching(true);
      const results = await invoke('api_search_transcripts', { query }) as TranscriptSearchResult[];
      setSearchResults(results);
    } catch (error) {
      console.error('Error searching transcripts:', error);
      setSearchResults([]);
    } finally {
      setIsSearching(false);
    }
  };

  const startSummaryPolling = React.useCallback((
    meetingId: string,
    processId: string,
    onUpdate: (result: any) => void
  ) => {
    if (activeSummaryPolls.has(meetingId)) {
      clearInterval(activeSummaryPolls.get(meetingId)!);
    }

    console.log(`📊 Starting polling for meeting ${meetingId}, process ${processId}`);

    let pollCount = 0;
    const MAX_POLLS = 200;

    const pollInterval = setInterval(async () => {
      pollCount++;

      if (pollCount >= MAX_POLLS) {
        console.warn(`⏱️ Polling timeout for ${meetingId} after ${MAX_POLLS} iterations`);
        clearInterval(pollInterval);
        setActiveSummaryPolls(prev => {
          const next = new Map(prev);
          next.delete(meetingId);
          return next;
        });
        onUpdate({
          status: 'error',
          error: 'Summary generation timed out after 15 minutes. Please try again or check your model configuration.'
        });
        return;
      }
      try {
        const result = await invoke('api_get_summary', {
          meetingId: meetingId,
        }) as any;

        console.log(`📊 Polling update for ${meetingId}:`, result.status);

        onUpdate(result);

        if (result.status === 'completed' || result.status === 'error' || result.status === 'failed' || result.status === 'cancelled') {
          console.log(`Polling completed for ${meetingId}, status: ${result.status}`);
          clearInterval(pollInterval);
          setActiveSummaryPolls(prev => {
            const next = new Map(prev);
            next.delete(meetingId);
            return next;
          });
        } else if (result.status === 'idle' && pollCount > 1) {
          console.log(`Process completed or not found for ${meetingId}, stopping poll`);
          clearInterval(pollInterval);
          setActiveSummaryPolls(prev => {
            const next = new Map(prev);
            next.delete(meetingId);
            return next;
          });
        }
      } catch (error) {
        console.error(`Polling error for ${meetingId}:`, error);
        onUpdate({
          status: 'error',
          error: error instanceof Error ? error.message : 'Unknown error'
        });
        clearInterval(pollInterval);
        setActiveSummaryPolls(prev => {
          const next = new Map(prev);
          next.delete(meetingId);
          return next;
        });
      }
    }, 5000);

    setActiveSummaryPolls(prev => new Map(prev).set(meetingId, pollInterval));
  }, [activeSummaryPolls]);

  const stopSummaryPolling = React.useCallback((meetingId: string) => {
    const pollInterval = activeSummaryPolls.get(meetingId);
    if (pollInterval) {
      console.log(`⏹️ Stopping polling for ${meetingId}`);
      clearInterval(pollInterval);
      setActiveSummaryPolls(prev => {
        const next = new Map(prev);
        next.delete(meetingId);
        return next;
      });
    }
  }, [activeSummaryPolls]);

  useEffect(() => {
    return () => {
      console.log('🧹 Cleaning up all summary polling intervals');
      activeSummaryPolls.forEach(interval => clearInterval(interval));
    };
  }, [activeSummaryPolls]);



  return (
    <SidebarContext.Provider value={{
      currentMeeting,
      setCurrentMeeting,
      sidebarItems,
      isCollapsed,
      toggleCollapse,
      meetings,
      setMeetings,
      isMeetingActive,
      setIsMeetingActive,
      handleRecordingToggle,
      searchTranscripts,
      searchResults,
      isSearching,
      setServerAddress,
      serverAddress,
      transcriptServerAddress,
      setTranscriptServerAddress,
      activeSummaryPolls,
      startSummaryPolling,
      stopSummaryPolling,
      refetchMeetings: fetchMeetings,
      folders,
      refetchFolders: fetchFolders,
      createFolder,
      renameFolder,
      deleteFolder,
      assignMeetingToFolder,
      aiCategorizeMeeting,
      aiCategorizeAllMeetings,
    }}>
      {children}
    </SidebarContext.Provider>
  );
}
