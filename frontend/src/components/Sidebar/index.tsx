'use client';

import React, { useState, useMemo, useEffect, useCallback } from 'react';
import { ChevronDown, ChevronRight, File, Settings, ChevronLeftCircle, ChevronRightCircle, Calendar, StickyNote, Home, Trash2, Mic, Square, Plus, Search, Pencil, NotebookPen, SearchIcon, X, Upload, Folder, FolderPlus, Sparkles, MoreVertical } from 'lucide-react';
import { useRouter, usePathname } from 'next/navigation';
import { useSidebar } from './SidebarProvider';
import { FOLDER_ROOT_ID, type SidebarItem } from './folderTree';
import { ConfirmationModal } from '../ConfirmationModel/confirmation-modal';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { SettingTabs } from '../SettingTabs';
import { TranscriptModelProps } from '@/components/TranscriptSettings';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { toast } from 'sonner';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { useConfig } from '@/contexts/ConfigContext';

import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"

import { MessageToast } from '../MessageToast';
import Logo from '../Logo';
import { GlobalAskPanel } from './GlobalAskPanel';
import Info from '../Info';
import { ComplianceNotification } from '../ComplianceNotification';
import { Input } from '../ui/input';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '../ui/input-group';

const Sidebar: React.FC = () => {
  const router = useRouter();
  const pathname = usePathname();
  const {
    currentMeeting,
    setCurrentMeeting,
    sidebarItems,
    isCollapsed,
    toggleCollapse,
    handleRecordingToggle,
    searchTranscripts,
    searchResults,
    isSearching,
    meetings,
    setMeetings,
    serverAddress,
    createFolder,
    renameFolder,
    deleteFolder,
    assignMeetingToFolder,
    aiCategorizeMeeting,
    aiCategorizeAllMeetings,
  } = useSidebar();

  // Get recording state from RecordingStateContext (single source of truth)
  const { isRecording } = useRecordingState();
  const { openImportDialog } = useImportDialog();
  const { betaFeatures } = useConfig();
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set([FOLDER_ROOT_ID]));
  const [searchQuery, setSearchQuery] = useState<string>('');
  const [folderContextMenu, setFolderContextMenu] = useState<{
    folderId: string;
    folderName: string;
    x: number;
    y: number;
  } | null>(null);
  const [createFolderModalOpen, setCreateFolderModalOpen] = useState(false);
  const [newFolderName, setNewFolderName] = useState('');
  const [renameFolderModal, setRenameFolderModal] = useState<{
    folderId: string;
    name: string;
  } | null>(null);
  const [draggedMeetingId, setDraggedMeetingId] = useState<string | null>(null);
  const [dropTargetFolderId, setDropTargetFolderId] = useState<string | null>(null);
  const [showModelSettings, setShowModelSettings] = useState(false);
  const [modelConfig, setModelConfig] = useState<ModelConfig>({
    provider: 'ollama',
    model: '',
    whisperModel: '',
    apiKey: null,
    ollamaEndpoint: null
  });
  const [transcriptModelConfig, setTranscriptModelConfig] = useState<TranscriptModelProps>({
    provider: 'parakeet',
    model: 'parakeet-tdt-0.6b-v3-int8',
  });
  const [settingsSaveSuccess, setSettingsSaveSuccess] = useState<boolean | null>(null);

  // State for edit modal
  const [editModalState, setEditModalState] = useState<{ isOpen: boolean; meetingId: string | null; currentTitle: string }>({
    isOpen: false,
    meetingId: null,
    currentTitle: ''
  });
  const [editingTitle, setEditingTitle] = useState<string>('');

  // Ensure root 'Meeting Notes' folder is always expanded
  useEffect(() => {
    if (!expandedFolders.has(FOLDER_ROOT_ID)) {
      const newExpanded = new Set(expandedFolders);
      newExpanded.add(FOLDER_ROOT_ID);
      setExpandedFolders(newExpanded);
    }
  }, [expandedFolders]);

  // useEffect(() => {
  //   if (settingsSaveSuccess !== null) {
  //     const timer = setTimeout(() => {
  //       setSettingsSaveSuccess(null);
  //     }, 3000);
  //   }
  // }, [settingsSaveSuccess]);


  const [deleteModalState, setDeleteModalState] = useState<{ isOpen: boolean; itemId: string | null }>({ isOpen: false, itemId: null });

  useEffect(() => {
    // Note: Don't set hardcoded defaults - let DB be the source of truth
    const fetchModelConfig = async () => {
      // Only make API call if serverAddress is loaded
      if (!serverAddress) {
        console.log('Waiting for server address to load before fetching model config');
        return;
      }

      try {
        const data = await invoke('api_get_model_config') as any;
        if (data && data.provider !== null) {
          // Fetch API key if not included and provider requires it
          if (data.provider !== 'ollama' && !data.apiKey) {
            try {
              const apiKeyData = await invoke('api_get_api_key', {
                provider: data.provider
              }) as string;
              data.apiKey = apiKeyData;
            } catch (err) {
              console.error('Failed to fetch API key:', err);
            }
          }
          setModelConfig(data);
        }
      } catch (error) {
        console.error('Failed to fetch model config:', error);
      }
    };

    fetchModelConfig();
  }, [serverAddress]);


  useEffect(() => {
    // Note: Don't set hardcoded defaults - let DB be the source of truth
    const fetchTranscriptSettings = async () => {
      // Only make API call if serverAddress is loaded
      if (!serverAddress) {
        console.log('Waiting for server address to load before fetching transcript settings');
        return;
      }

      try {
        const data = await invoke('api_get_transcript_config') as any;
        if (data && data.provider !== null) {
          setTranscriptModelConfig(data);
        }
      } catch (error) {
        console.error('Failed to fetch transcript settings:', error);
      }
    };
    fetchTranscriptSettings();
  }, [serverAddress]);

  // Listen for model config updates from other components
  useEffect(() => {
    const setupListener = async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const unlisten = await listen<ModelConfig>('model-config-updated', (event) => {
        console.log('Sidebar received model-config-updated event:', event.payload);
        setModelConfig(event.payload);
      });

      return unlisten;
    };

    let cleanup: (() => void) | undefined;
    setupListener().then(fn => cleanup = fn);

    return () => {
      cleanup?.();
    };
  }, []);



  // Handle model config save
  const handleSaveModelConfig = async (config: ModelConfig) => {
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey,
        ollamaEndpoint: config.ollamaEndpoint,
      });

      setModelConfig(config);
      console.log('Model config saved successfully');
      setSettingsSaveSuccess(true);

      // Emit event to sync other components
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      // Track settings change
      await Analytics.trackSettingsChanged('model_config', `${config.provider}_${config.model}`);
    } catch (error) {
      console.error('Error saving model config:', error);
      setSettingsSaveSuccess(false);
    }
  };

  const handleSaveTranscriptConfig = async (updatedConfig?: TranscriptModelProps) => {
    try {
      const configToSave = updatedConfig || transcriptModelConfig;
      const payload = {
        provider: configToSave.provider,
        model: configToSave.model,
        apiKey: configToSave.apiKey ?? null
      };
      console.log('Saving transcript config with payload:', payload);

      await invoke('api_save_transcript_config', {
        provider: payload.provider,
        model: payload.model,
        apiKey: payload.apiKey,
      });


      setSettingsSaveSuccess(true);

      // Track settings change
      const transcriptConfigToSave = updatedConfig || transcriptModelConfig;
      await Analytics.trackSettingsChanged('transcript_config', `${transcriptConfigToSave.provider}_${transcriptConfigToSave.model}`);
    } catch (error) {
      console.error('Failed to save transcript config:', error);
      setSettingsSaveSuccess(false);
    }
  };

  // Handle search input changes
  const handleSearchChange = useCallback(async (value: string) => {
    setSearchQuery(value);

    // If search query is empty, just return to normal view
    if (!value.trim()) return;

    // Search through transcripts
    await searchTranscripts(value);

    // Make sure the root folder is expanded when searching
    if (!expandedFolders.has(FOLDER_ROOT_ID)) {
      const newExpanded = new Set(expandedFolders);
      newExpanded.add(FOLDER_ROOT_ID);
      setExpandedFolders(newExpanded);
    }
  }, [expandedFolders, searchTranscripts]);

  // Combine search results with sidebar items
  const filteredSidebarItems = useMemo(() => {
    if (!searchQuery.trim()) return sidebarItems;

    // If we have search results, highlight matching meetings
    if (searchResults.length > 0) {
      // Get the IDs of meetings that matched in transcripts
      const matchedMeetingIds = new Set(searchResults.map(result => result.id));

      return sidebarItems
        .map(folder => {
          // Always include folders in the results
          if (folder.type === 'folder') {
            if (!folder.children) return folder;

            // Filter children based on search results or title match
            const filteredChildren = folder.children.filter(item => {
              // Include if the meeting ID is in our search results
              if (item.meetingId && matchedMeetingIds.has(item.meetingId)) return true;

              // Or if the title matches the search query
              return item.title.toLowerCase().includes(searchQuery.toLowerCase());
            });

            return {
              ...folder,
              children: filteredChildren
            };
          }

          // For non-folder items, check if they match the search
          return ((folder.meetingId && matchedMeetingIds.has(folder.meetingId)) ||
            folder.title.toLowerCase().includes(searchQuery.toLowerCase()))
            ? folder : undefined;
        })
        .filter((item): item is SidebarItem => item !== undefined); // Type-safe filter
    } else {
      // Fall back to title-only filtering if no transcript results
      return sidebarItems
        .map(folder => {
          // Always include folders in the results
          if (folder.type === 'folder') {
            if (!folder.children) return folder;

            // Filter children based on search query
            const filteredChildren = folder.children.filter(item =>
              item.title.toLowerCase().includes(searchQuery.toLowerCase())
            );

            return {
              ...folder,
              children: filteredChildren
            };
          }

          // For non-folder items, check if they match the search
          return folder.title.toLowerCase().includes(searchQuery.toLowerCase()) ? folder : undefined;
        })
        .filter((item): item is SidebarItem => item !== undefined); // Type-safe filter
    }
  }, [sidebarItems, searchQuery, searchResults, expandedFolders]);


  const handleDelete = async (itemId: string) => {
    console.log('Deleting item:', itemId);
    const payload = {
      meetingId: itemId
    };

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('api_delete_meeting', {
        meetingId: itemId,
      });
      console.log('Meeting deleted successfully');
      const updatedMeetings = meetings.filter((m) => m.id !== itemId);
      setMeetings(updatedMeetings);

      // Track meeting deletion
      Analytics.trackMeetingDeleted(itemId);

      // Show success toast
      toast.success("Meeting deleted successfully", {
        description: "All associated data has been removed"
      });

      // If deleting the active meeting, navigate to home
      if (currentMeeting?.id === itemId) {
        setCurrentMeeting({ id: 'intro-call', title: '+ New Call' });
        router.push('/');
      }
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      toast.error("Failed to delete meeting", {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const handleDeleteConfirm = () => {
    if (deleteModalState.itemId) {
      handleDelete(deleteModalState.itemId);
    }
    setDeleteModalState({ isOpen: false, itemId: null });
  };

  // Handle modal editing of meeting names
  const handleEditStart = (meetingId: string, currentTitle: string) => {
    setEditModalState({
      isOpen: true,
      meetingId: meetingId,
      currentTitle: currentTitle
    });
    setEditingTitle(currentTitle);
  };

  const handleEditConfirm = async () => {
    const newTitle = editingTitle.trim();
    const meetingId = editModalState.meetingId;

    if (!meetingId) return;

    // Prevent empty titles
    if (!newTitle) {
      toast.error("Meeting title cannot be empty");
      return;
    }

    try {
      await invoke('api_save_meeting_title', {
        meetingId: meetingId,
        title: newTitle,
      });

      // Update local state
      const updatedMeetings = meetings.map((m) =>
        m.id === meetingId ? { ...m, title: newTitle } : m
      );
      setMeetings(updatedMeetings);

      // Update current meeting if it's the one being edited
      if (currentMeeting?.id === meetingId) {
        setCurrentMeeting({ id: meetingId, title: newTitle });
      }

      // Track the edit
      Analytics.trackButtonClick('edit_meeting_title', 'sidebar');

      toast.success("Meeting title updated successfully");

      // Close modal and reset state
      setEditModalState({ isOpen: false, meetingId: null, currentTitle: '' });
      setEditingTitle('');
    } catch (error) {
      console.error('Failed to update meeting title:', error);
      toast.error("Failed to update meeting title", {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const handleEditCancel = () => {
    setEditModalState({ isOpen: false, meetingId: null, currentTitle: '' });
    setEditingTitle('');
  };

  const handleCreateFolderConfirm = async () => {
    const name = newFolderName.trim();
    if (!name) {
      toast.error('Folder name cannot be empty');
      return;
    }
    try {
      await createFolder(name);
      setCreateFolderModalOpen(false);
      setNewFolderName('');
      toast.success(`Created folder "${name}"`);
    } catch (err) {
      toast.error('Failed to create folder', {
        description: err instanceof Error ? err.message : String(err),
      });
    }
  };

  const handleRenameFolderConfirm = async () => {
    if (!renameFolderModal) return;
    const name = renameFolderModal.name.trim();
    if (!name) {
      toast.error('Folder name cannot be empty');
      return;
    }
    try {
      await renameFolder(renameFolderModal.folderId, name);
      setRenameFolderModal(null);
      toast.success('Folder renamed');
    } catch (err) {
      toast.error('Failed to rename folder', {
        description: err instanceof Error ? err.message : String(err),
      });
    }
  };

  const toggleFolder = (folderId: string) => {
    // Normal toggle behavior for all folders
    const newExpanded = new Set(expandedFolders);
    if (newExpanded.has(folderId)) {
      newExpanded.delete(folderId);
    } else {
      newExpanded.add(folderId);
    }
    setExpandedFolders(newExpanded);
  };

  // Expose setShowModelSettings to window for Rust tray to call
  useEffect(() => {
    (window as any).openSettings = () => {
      setShowModelSettings(true);
    };

    // Cleanup on unmount
    return () => {
      delete (window as any).openSettings;
    };
  }, []);

  useEffect(() => {
    const handleShortcut = (e: KeyboardEvent) => {
      if (e.key === ',' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        router.push('/settings');
      }
    };
    window.addEventListener('keydown', handleShortcut);
    return () => window.removeEventListener('keydown', handleShortcut);
  }, [router]);

  const collapsedNavTileClass = (active: boolean) =>
    `flex h-10 w-10 items-center justify-center rounded-xl border transition-colors duration-150 ${active
      ? 'border-border/10 bg-secondary/[.09] text-foreground'
      : 'border-transparent text-muted-foreground hover:bg-secondary/5 hover:text-foreground'
    }`;

  const footerButtonClass = 'flex w-full items-center justify-center gap-2 rounded-xl border border-border/10 bg-secondary/5 px-3 py-2 text-sm font-medium text-foreground hover:bg-secondary/10 transition-colors';

  const renderCollapsedIcons = () => {
    if (!isCollapsed) return null;

    const isHomePage = pathname === '/';
    const isMeetingPage = pathname?.includes('/meeting-details');
    const isSettingsPage = pathname === '/settings';

    return (
      <TooltipProvider>
        <div className="flex h-full flex-col items-center gap-[26px] px-2 pb-5 pt-5">
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={toggleCollapse}
                className="flex h-[34px] w-[34px] items-center justify-center rounded-lg border border-border/10 bg-gradient-to-br from-white/10 to-white/0 text-foreground transition-opacity hover:opacity-80"
              >
                <ChevronRightCircle className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Expand sidebar</p>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={handleRecordingToggle}
                disabled={isRecording}
                className={`flex h-12 w-12 items-center justify-center rounded-2xl border transition-all duration-150 disabled:cursor-not-allowed ${isRecording
                  ? 'border-destructive/40 bg-destructive/15 shadow-[0_0_22px_-4px_hsl(var(--destructive)/0.6)]'
                  : 'border-border/10 bg-secondary/5 hover:bg-secondary/10'
                  }`}
              >
                {isRecording ? (
                  <Square className="w-5 h-5 text-destructive" />
                ) : (
                  <Mic className="w-5 h-5 text-foreground" />
                )}
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>{isRecording ? "Recording in progress..." : "Start Recording"}</p>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => router.push('/')}
                className={collapsedNavTileClass(isHomePage)}
              >
                <Home className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Home</p>
            </TooltipContent>
          </Tooltip>

          {betaFeatures.importAndRetranscribe && (
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => openImportDialog()}
                  className={collapsedNavTileClass(false)}
                >
                  <Upload className="w-5 h-5" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>Import Audio</p>
              </TooltipContent>
            </Tooltip>
          )}

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => {
                  if (isCollapsed) toggleCollapse();
                  toggleFolder(FOLDER_ROOT_ID);
                }}
                className={collapsedNavTileClass(isMeetingPage)}
              >
                <NotebookPen className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Meeting Notes</p>
            </TooltipContent>
          </Tooltip>

          <div className="flex-1" />

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => router.push('/settings')}
                className={`flex h-10 w-10 items-center justify-center rounded-xl transition-colors duration-150 ${isSettingsPage ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'
                  }`}
              >
                <Settings className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Settings</p>
            </TooltipContent>
          </Tooltip>

          <Info isCollapsed={isCollapsed} />
        </div>
      </TooltipProvider>
    );
  };

  // Find matching transcript snippet for a meeting item
  const findMatchingSnippet = (itemId: string) => {
    if (!searchQuery.trim() || !searchResults.length) return null;
    return searchResults.find(result => result.id === itemId);
  };

  const renderItem = (item: SidebarItem, depth = 0) => {
    const isExpanded = expandedFolders.has(item.id);
    const paddingLeft = `${depth * 12 + 12}px`;
    const isActive = item.type === 'file' && currentMeeting?.id === item.meetingId;
    const isMeetingItem = item.type === 'file' && !!item.meetingId;
    const isUserFolder = item.type === 'folder' && !!item.folderId && item.id !== 'unfiled';
    const isDropTarget = isUserFolder && dropTargetFolderId === item.folderId;

    // Check if this item has a matching transcript snippet
    const matchingResult = isMeetingItem && item.meetingId ? findMatchingSnippet(item.meetingId) : null;
    const hasTranscriptMatch = !!matchingResult;

    if (isCollapsed) return null;

    const leafRowClass = isActive
      ? 'bg-primary/10 text-foreground font-medium'
      : hasTranscriptMatch
        ? 'bg-accent-violet/10 text-foreground'
        : 'bg-secondary/5 text-muted-foreground hover:bg-secondary/10 hover:text-foreground';

    return (
      <div key={item.id}>
        <div
          className={`flex items-center transition-all duration-150 group ${item.type === 'folder' && depth === 0
            ? 'p-3 text-base font-semibold h-10 mx-3 mt-3 rounded-xl text-foreground'
            : `px-3 py-2 my-1 rounded-lg text-sm cursor-pointer transition-colors duration-150 ${leafRowClass} ${isDropTarget ? 'ring-2 ring-primary/60 bg-primary/5' : ''}`
            }`}
          style={item.type === 'folder' && depth === 0 ? {} : { paddingLeft }}
          draggable={isMeetingItem}
          onDragStart={(e) => {
            if (isMeetingItem && item.meetingId) {
              setDraggedMeetingId(item.meetingId);
              e.dataTransfer.effectAllowed = 'move';
              e.dataTransfer.setData('text/plain', item.meetingId);
            }
          }}
          onDragEnd={() => {
            setDraggedMeetingId(null);
            setDropTargetFolderId(null);
          }}
          onDragOver={(e) => {
            if (isUserFolder && draggedMeetingId) {
              e.preventDefault();
              e.dataTransfer.dropEffect = 'move';
              if (dropTargetFolderId !== item.folderId) {
                setDropTargetFolderId(item.folderId ?? null);
              }
            }
          }}
          onDragLeave={() => {
            if (isUserFolder && dropTargetFolderId === item.folderId) {
              setDropTargetFolderId(null);
            }
          }}
          onDrop={async (e) => {
            e.preventDefault();
            const meetingId = e.dataTransfer.getData('text/plain') || draggedMeetingId;
            if (isUserFolder && item.folderId && meetingId) {
              try {
                await assignMeetingToFolder(meetingId, item.folderId);
                toast.success(`Moved to ${item.title}`);
              } catch (err) {
                toast.error('Failed to move meeting', {
                  description: err instanceof Error ? err.message : String(err),
                });
              }
            }
            setDraggedMeetingId(null);
            setDropTargetFolderId(null);
          }}
          onClick={() => {
            if (item.type === 'folder') {
              toggleFolder(item.id);
            } else {
              if (!item.meetingId) {
                router.push('/');
                return;
              }
              setCurrentMeeting({ id: item.meetingId, title: item.title });
              router.push(`/meeting-details?id=${item.meetingId}`);
            }
          }}
        >
          {item.type === 'folder' ? (
            <>
              {depth === 0 ? (
                <NotebookPen className="w-4 h-4 mr-2 text-muted-foreground" />
              ) : isUserFolder ? (
                <Folder className="w-4 h-4 mr-2 text-muted-foreground" />
              ) : (
                <Calendar className="w-4 h-4 mr-2 text-muted-foreground" />
              )}
              <span className={depth === 0 ? "" : "font-medium"}>{item.title}</span>
              <div className="ml-auto flex items-center gap-1">
                {isUserFolder && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      setFolderContextMenu({
                        folderId: item.folderId!,
                        folderName: item.title,
                        x: e.clientX,
                        y: e.clientY,
                      });
                    }}
                    className="opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-foreground p-1 rounded-md hover:bg-secondary/10 flex-shrink-0"
                    aria-label="Folder actions"
                  >
                    <MoreVertical className="w-3.5 h-3.5" />
                  </button>
                )}
                {isExpanded ? (
                  <ChevronDown className="w-4 h-4 text-muted-foreground" />
                ) : (
                  <ChevronRight className="w-4 h-4 text-muted-foreground" />
                )}
              </div>
              {searchQuery && item.id === FOLDER_ROOT_ID && isSearching && (
                <span className="ml-2 text-xs text-primary animate-pulse">Searching...</span>
              )}
            </>
          ) : (
            <div className="flex flex-col w-full">
              <div className="flex items-center w-full">
                {isMeetingItem ? (
                  <div className="flex-shrink-0 flex items-center justify-center w-6 h-6 rounded-full mr-2 bg-secondary/10">
                    <File className="w-3.5 h-3.5 text-muted-foreground" />
                  </div>
                ) : (
                  <div className="flex-shrink-0 flex items-center justify-center w-6 h-6 rounded-full mr-2 bg-primary/15">
                    <Plus className="w-3.5 h-3.5 text-primary" />
                  </div>
                )}
                <span className="flex-1 break-words">{item.title}</span>
                {isMeetingItem && item.meetingId && (
                  <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        handleEditStart(item.meetingId!, item.title);
                      }}
                      className="text-muted-foreground hover:text-primary p-1 rounded-md hover:bg-primary/10 flex-shrink-0"
                      aria-label="Edit meeting title"
                    >
                      <Pencil className="w-4 h-4" />
                    </button>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        setDeleteModalState({ isOpen: true, itemId: item.meetingId! });
                      }}
                      className="text-muted-foreground hover:text-destructive p-1 rounded-md hover:bg-destructive/10 flex-shrink-0"
                      aria-label="Delete meeting"
                    >
                      <Trash2 className="w-4 h-4" />
                    </button>
                  </div>
                )}
              </div>

              {/* Show transcript match snippet if available */}
              {hasTranscriptMatch && (
                <div className="mt-1 ml-8 text-xs text-muted-foreground bg-accent-violet/10 p-1.5 rounded-md border border-accent-violet/20 line-clamp-2">
                  <span className="font-medium text-accent-violet">Match:</span> {matchingResult.matchContext}
                </div>
              )}
            </div>
          )}
        </div>
        {item.type === 'folder' && isExpanded && item.children && (
          <div className="ml-1">
            {item.children.map(child => renderItem(child, depth + 1))}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="fixed left-3 top-3 bottom-3 z-40">
      {!isCollapsed && (
        <button
          onClick={toggleCollapse}
          className="glass-pill absolute -right-3 top-20 z-50 p-1.5 text-foreground hover:bg-secondary/20 transition-colors shadow-lg"
          style={{ transform: 'translateX(50%)' }}
        >
          <ChevronLeftCircle className="w-6 h-6" />
        </button>
      )}

      <div
        className={`glass-rail h-full flex flex-col transition-all duration-300 ${isCollapsed ? 'w-20' : 'w-64'
          }`}
      >
        {/*  Header with traffic light spacing */}
        <div className={`flex-shrink-0 flex items-center ${isCollapsed ? '' : 'h-22'}`}>

          {/* Title container */}

          <div className="flex-1">
            {!isCollapsed && (
              <div className="p-3">
                <Logo isCollapsed={isCollapsed} />

                <div className="relative mb-1">
                  <InputGroup >
                    <InputGroupInput placeholder='Search meeting content...' value={searchQuery}
                      onChange={(e) => handleSearchChange(e.target.value)}
                    />
                    <InputGroupAddon>
                      <SearchIcon />
                    </InputGroupAddon>
                    {searchQuery &&
                      <InputGroupAddon align={'inline-end'}>
                        <InputGroupButton
                          onClick={() => handleSearchChange('')}
                        >
                          <X />
                        </InputGroupButton>
                      </InputGroupAddon>
                    }
                  </InputGroup>
                </div>

                <GlobalAskPanel />
              </div>
            )}
          </div>
        </div>

        {/* Main content - scrollable area */}
        <div className="flex-1 flex flex-col min-h-0">
          {/* Fixed navigation items */}
          <div className="flex-shrink-0">
            {!isCollapsed && (
              <div
                onClick={() => router.push('/')}
                className="flex h-10 items-center gap-2 rounded-xl mx-3 mt-3 px-3 text-base font-semibold text-foreground hover:bg-secondary/10 transition-colors cursor-pointer"
              >
                <Home className="w-4 h-4" />
                <span>Home</span>
              </div>
            )}
          </div>

          {/* Content area */}
          <div className="flex-1 flex flex-col min-h-0">
            {renderCollapsedIcons()}
            {/* Folder tree header with create button */}
            {!isCollapsed && (
              <div className="flex-shrink-0 flex items-center mx-3 mt-3 px-1">
                <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                  Folders
                </span>
                <div className="ml-auto flex items-center gap-1">
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={() => {
                            setCreateFolderModalOpen(true);
                            setNewFolderName('');
                          }}
                          className="text-muted-foreground hover:text-foreground p-1 rounded-md hover:bg-secondary/10"
                          aria-label="Create folder"
                        >
                          <FolderPlus className="w-4 h-4" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent side="right">
                        <p>New folder</p>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                </div>
              </div>
            )}

            {/* Scrollable tree */}
            {!isCollapsed && (
              <div
                className="flex-1 overflow-y-auto custom-scrollbar min-h-0"
                onClick={() => setFolderContextMenu(null)}
              >
                {filteredSidebarItems
                  .filter(item => item.type === 'folder' && item.id === FOLDER_ROOT_ID)
                  .map(root => (
                    <div key={root.id}>
                      {(root.children ?? []).map(child => renderItem(child, 1))}
                    </div>
                  ))}
              </div>
            )}
          </div>
        </div>

        {/* Footer */}
        {!isCollapsed && (

          <div className="flex-shrink-0 p-3 border-t border-border/10">
            <button
              onClick={handleRecordingToggle}
              disabled={isRecording}
              className={`flex w-full items-center justify-center gap-2 rounded-xl border px-3 py-2.5 text-sm font-medium transition-all duration-150 ${isRecording
                ? 'cursor-not-allowed border-destructive/40 bg-destructive/15 text-destructive shadow-[0_0_20px_-4px_hsl(var(--destructive)/0.5)]'
                : 'border-destructive/30 bg-destructive/10 text-destructive hover:bg-destructive/20'
                }`}
            >
              {isRecording ? (
                <>
                  <Square className="w-4 h-4" />
                  <span>Recording in progress...</span>
                </>
              ) : (
                <>
                  <Mic className="w-4 h-4" />
                  <span>Start Recording</span>
                </>
              )}
            </button>

            {betaFeatures.importAndRetranscribe && (
              <button
                onClick={() => openImportDialog()}
                className={`mt-2 ${footerButtonClass}`}
              >
                <Upload className="w-4 h-4" />
                <span>Import Audio</span>
              </button>
            )}

            <button
              onClick={() => router.push('/settings')}
              className={`mt-2 mb-1 ${footerButtonClass}`}
            >
              <Settings className="w-4 h-4" />
              <span>Settings</span>
            </button>
            <Info isCollapsed={isCollapsed} />
            <div className="w-full flex items-center justify-center px-3 py-1 text-xs font-mono text-muted-foreground">
              v0.4.0
            </div>
          </div>
        )}
      </div>

      {/* Confirmation Modal for Delete */}
      <ConfirmationModal
        isOpen={deleteModalState.isOpen}
        text="Are you sure you want to delete this meeting? This action cannot be undone."
        onConfirm={handleDeleteConfirm}
        onCancel={() => setDeleteModalState({ isOpen: false, itemId: null })}
      />

      {/* Edit Meeting Title Modal */}
      <Dialog open={editModalState.isOpen} onOpenChange={(open) => {
        if (!open) handleEditCancel();
      }}>
        <DialogContent className="sm:max-w-[425px]">
          <VisuallyHidden>
            <DialogTitle>Edit Meeting Title</DialogTitle>
          </VisuallyHidden>
          <div className="py-4">
            <h3 className="text-lg font-semibold text-foreground mb-4">Edit Meeting Title</h3>
            <div className="space-y-4">
              <div>
                <label htmlFor="meeting-title" className="block text-sm font-medium text-muted-foreground mb-2">
                  Meeting Title
                </label>
                <input
                  id="meeting-title"
                  type="text"
                  value={editingTitle}
                  onChange={(e) => setEditingTitle(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      handleEditConfirm();
                    } else if (e.key === 'Escape') {
                      handleEditCancel();
                    }
                  }}
                  className="w-full px-3 py-2 rounded-lg border border-border/10 bg-secondary/5 text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary/50 focus:border-transparent"
                  placeholder="Enter meeting title"
                  autoFocus
                />
              </div>
            </div>
          </div>
          <DialogFooter>
            <button
              onClick={handleEditCancel}
              className="px-4 py-2 text-sm font-medium text-foreground bg-secondary/5 border border-border/10 hover:bg-secondary/10 rounded-lg transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={handleEditConfirm}
              className="px-4 py-2 text-sm font-medium text-primary-foreground bg-primary hover:bg-primary/90 rounded-lg transition-colors"
            >
              Save
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Folder context menu */}
      {folderContextMenu && (
        <div
          className="fixed z-50 min-w-[180px] rounded-lg border border-border/10 bg-secondary/95 backdrop-blur shadow-xl py-1 text-sm"
          style={{ left: folderContextMenu.x, top: folderContextMenu.y }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            onClick={async () => {
              const meetingId = draggedMeetingId;
              const folderId = folderContextMenu.folderId;
              setFolderContextMenu(null);
              if (!meetingId) {
                toast.error('Drag a meeting onto a folder to assign it.');
                return;
              }
              try {
                await assignMeetingToFolder(meetingId, folderId);
                toast.success(`Moved to ${folderContextMenu.folderName}`);
              } catch (err) {
                toast.error('Failed to move meeting', {
                  description: err instanceof Error ? err.message : String(err),
                });
              }
            }}
            className="w-full text-left px-3 py-1.5 hover:bg-secondary/20 text-foreground"
            disabled={!draggedMeetingId}
          >
            Move dragged meeting here
          </button>
          <button
            onClick={async () => {
              setFolderContextMenu(null);
              try {
                const r = await aiCategorizeAllMeetings();
                toast.success(
                  `AI categorized ${r.assigned + r.suggested_new} meeting(s)` +
                    (r.failed ? `, ${r.failed} failed` : ''),
                );
              } catch (err) {
                toast.error('AI categorize failed', {
                  description: err instanceof Error ? err.message : String(err),
                });
              }
            }}
            className="w-full text-left px-3 py-1.5 hover:bg-secondary/20 text-foreground flex items-center gap-2"
          >
            <Sparkles className="w-3.5 h-3.5" />
            Auto-categorize unfiled
          </button>
          <div className="my-1 border-t border-border/10" />
          <button
            onClick={() => {
              setRenameFolderModal({
                folderId: folderContextMenu.folderId,
                name: folderContextMenu.folderName,
              });
              setFolderContextMenu(null);
            }}
            className="w-full text-left px-3 py-1.5 hover:bg-secondary/20 text-foreground"
          >
            Rename
          </button>
          <button
            onClick={async () => {
              const id = folderContextMenu.folderId;
              const name = folderContextMenu.folderName;
              setFolderContextMenu(null);
              if (!window.confirm(`Delete folder "${name}"? Meetings in it will become unfiled.`)) return;
              try {
                await deleteFolder(id);
                toast.success(`Deleted folder "${name}"`);
              } catch (err) {
                toast.error('Failed to delete folder', {
                  description: err instanceof Error ? err.message : String(err),
                });
              }
            }}
            className="w-full text-left px-3 py-1.5 hover:bg-destructive/15 text-destructive flex items-center gap-2"
          >
            <Trash2 className="w-3.5 h-3.5" />
            Delete
          </button>
        </div>
      )}

      {/* Create folder modal */}
      <Dialog open={createFolderModalOpen} onOpenChange={setCreateFolderModalOpen}>
        <DialogContent className="sm:max-w-[425px]">
          <VisuallyHidden>
            <DialogTitle>Create folder</DialogTitle>
          </VisuallyHidden>
          <div className="py-4">
            <h3 className="text-lg font-semibold text-foreground mb-4">New folder</h3>
            <div className="space-y-4">
              <div>
                <label htmlFor="folder-name" className="block text-sm font-medium text-muted-foreground mb-2">
                  Folder name
                </label>
                <input
                  id="folder-name"
                  type="text"
                  value={newFolderName}
                  onChange={(e) => setNewFolderName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void handleCreateFolderConfirm();
                    if (e.key === 'Escape') setCreateFolderModalOpen(false);
                  }}
                  className="w-full px-3 py-2 rounded-lg border border-border/10 bg-secondary/5 text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary/50 focus:border-transparent"
                  placeholder="e.g. Customer Calls"
                  autoFocus
                />
              </div>
            </div>
          </div>
          <DialogFooter>
            <button
              onClick={() => setCreateFolderModalOpen(false)}
              className="px-4 py-2 text-sm font-medium text-foreground bg-secondary/5 border border-border/10 hover:bg-secondary/10 rounded-lg transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={handleCreateFolderConfirm}
              className="px-4 py-2 text-sm font-medium text-primary-foreground bg-primary hover:bg-primary/90 rounded-lg transition-colors"
            >
              Create
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Rename folder modal */}
      <Dialog
        open={renameFolderModal !== null}
        onOpenChange={(open) => {
          if (!open) setRenameFolderModal(null);
        }}
      >
        <DialogContent className="sm:max-w-[425px]">
          <VisuallyHidden>
            <DialogTitle>Rename folder</DialogTitle>
          </VisuallyHidden>
          <div className="py-4">
            <h3 className="text-lg font-semibold text-foreground mb-4">Rename folder</h3>
            <div className="space-y-4">
              <div>
                <label htmlFor="rename-folder-name" className="block text-sm font-medium text-muted-foreground mb-2">
                  New name
                </label>
                <input
                  id="rename-folder-name"
                  type="text"
                  value={renameFolderModal?.name ?? ''}
                  onChange={(e) =>
                    setRenameFolderModal((prev) => (prev ? { ...prev, name: e.target.value } : prev))
                  }
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void handleRenameFolderConfirm();
                    if (e.key === 'Escape') setRenameFolderModal(null);
                  }}
                  className="w-full px-3 py-2 rounded-lg border border-border/10 bg-secondary/5 text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary/50 focus:border-transparent"
                  autoFocus
                />
              </div>
            </div>
          </div>
          <DialogFooter>
            <button
              onClick={() => setRenameFolderModal(null)}
              className="px-4 py-2 text-sm font-medium text-foreground bg-secondary/5 border border-border/10 hover:bg-secondary/10 rounded-lg transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={handleRenameFolderConfirm}
              className="px-4 py-2 text-sm font-medium text-primary-foreground bg-primary hover:bg-primary/90 rounded-lg transition-colors"
            >
              Save
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

export default Sidebar;
