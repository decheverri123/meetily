"use client";

import { useState, useCallback } from 'react';
import { Button } from '@/components/ui/button';
import { Copy, FolderOpen, RefreshCw } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { RetranscribeDialog } from './RetranscribeDialog';
import { useConfig } from '@/contexts/ConfigContext';
import { compactTileClass } from './toolbarStyles';
import { cn } from '@/lib/utils';


interface TranscriptButtonGroupProps {
  transcriptCount: number;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
}


export function TranscriptButtonGroup({
  transcriptCount,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
}: TranscriptButtonGroupProps) {
  const { betaFeatures } = useConfig();
  const [showRetranscribeDialog, setShowRetranscribeDialog] = useState(false);

  const handleRetranscribeComplete = useCallback(async () => {
    if (onRefetchTranscripts) {
      await onRefetchTranscripts();
    }
  }, [onRefetchTranscripts]);

  return (
    <div className="ml-auto flex shrink-0 items-center gap-1.5">
      <Button
        variant="ghost"
        size="icon"
        className={compactTileClass}
        onClick={() => {
          Analytics.trackButtonClick('copy_transcript', 'meeting_details');
          onCopyTranscript();
        }}
        disabled={transcriptCount === 0}
        title={transcriptCount === 0 ? 'No transcript available' : 'Copy Transcript'}
        aria-label={transcriptCount === 0 ? 'No transcript available' : 'Copy Transcript'}
      >
        <Copy />
      </Button>

      <Button
        variant="ghost"
        size="icon"
        className={compactTileClass}
        onClick={() => {
          Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
          onOpenMeetingFolder();
        }}
        title="Open Recording Folder"
        aria-label="Open Recording Folder"
      >
        <FolderOpen />
      </Button>

      {betaFeatures.importAndRetranscribe && meetingId && meetingFolderPath && (
        <>
          <Button
            variant="ghost"
            size="icon"
            className={cn(compactTileClass, 'border-accent-violet/25 bg-gradient-to-br from-accent-violet/20 to-primary/20 text-foreground hover:from-accent-violet/30 hover:to-primary/30')}
            onClick={() => {
              Analytics.trackButtonClick('enhance_transcript', 'meeting_details');
              setShowRetranscribeDialog(true);
            }}
            title="Retranscribe to enhance your recorded audio"
            aria-label="Retranscribe to enhance your recorded audio"
          >
            <RefreshCw />
          </Button>

          <RetranscribeDialog
            open={showRetranscribeDialog}
            onOpenChange={setShowRetranscribeDialog}
            meetingId={meetingId}
            meetingFolderPath={meetingFolderPath}
            onComplete={handleRetranscribeComplete}
          />
        </>
      )}
    </div>
  );
}
