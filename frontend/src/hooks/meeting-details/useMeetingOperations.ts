import { useCallback } from 'react';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';

interface UseMeetingOperationsProps {
  meeting: any;
}

export function useMeetingOperations({
  meeting,
}: UseMeetingOperationsProps) {

  // Open meeting folder in file explorer
  const handleOpenMeetingFolder = useCallback(async () => {
    try {
      await invokeTauri('open_meeting_folder', { meetingId: meeting.id });
    } catch (error) {
      console.error('Failed to open meeting folder:', error);
      toast.error(error as string || 'Failed to open recording folder');
    }
  }, [meeting.id]);

  // Delete meeting
  const handleDeleteMeeting = useCallback(async () => {
    try {
      await invokeTauri('api_delete_meeting', { meetingId: meeting.id });
      toast.success('Meeting deleted successfully');
      return true;
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      toast.error(typeof error === 'string' ? error : 'Failed to delete meeting');
      return false;
    }
  }, [meeting.id]);

  return {
    handleOpenMeetingFolder,
    handleDeleteMeeting,
  };
}
