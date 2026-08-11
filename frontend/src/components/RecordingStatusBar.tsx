'use client';

import { motion } from 'framer-motion';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useEffect, useState } from 'react';
import { cn } from '@/lib/utils';

interface RecordingStatusBarProps {
  isPaused?: boolean;
}

export const RecordingStatusBar: React.FC<RecordingStatusBarProps> = ({ isPaused = false }) => {
  // Get recording duration from backend-synced context (in seconds)
  // Backend polls every 500ms, providing smooth updates
  const { activeDuration, isRecording } = useRecordingState();

  // Display state synced from backend
  const [displaySeconds, setDisplaySeconds] = useState(0);

  // Sync with backend duration when it changes (handles refresh/navigation)
  useEffect(() => {
    if (activeDuration !== null) {
      // Round to nearest second to avoid decimal issues
      setDisplaySeconds(Math.floor(activeDuration));
    }
  }, [activeDuration]);

  const formatDuration = (seconds: number): string => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: -10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -10 }}
      transition={{ duration: 0.2 }}
      className={cn(
        'glass-pill inline-flex w-fit items-center gap-2 px-3 py-1.5 mb-2',
        isPaused ? 'border-border/10' : 'border-destructive/30 bg-destructive/15'
      )}
    >
      <div className={cn('w-2 h-2 rounded-full', isPaused ? 'bg-muted-foreground' : 'bg-destructive animate-rec-pulse')} />
      <span className={cn('text-sm font-mono', isPaused ? 'text-muted-foreground' : 'text-destructive')}>
        {isPaused ? 'Paused' : 'Recording'} · {formatDuration(displaySeconds)}
      </span>
    </motion.div>
  );
};
