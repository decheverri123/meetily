"use client";

import { Button } from '@/components/ui/button';
import { headerTileClass } from './toolbarStyles';
import { Copy, Save, Loader2, Search, FolderOpen, Trash2 } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { cn } from '@/lib/utils';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onCopy: () => Promise<void>;
  onFind?: () => void;
  onOpenFolder: () => Promise<void>;
  onDelete?: () => void;
  hasSummary: boolean;
}

export function SummaryUpdaterButtonGroup({
  isSaving,
  isDirty,
  onSave,
  onCopy,
  onFind,
  onOpenFolder,
  onDelete,
  hasSummary
}: SummaryUpdaterButtonGroupProps) {
  return (
    <div className="flex items-center gap-1.5">
      <Button
        variant="ghost"
        size="icon"
        className={cn(headerTileClass, isDirty && 'border-success/30 bg-success/20 text-success hover:bg-success/30')}
        title={isSaving ? "Saving" : "Save Changes"}
        aria-label={isSaving ? "Saving" : "Save Changes"}
        onClick={() => {
          Analytics.trackButtonClick('save_changes', 'meeting_details');
          onSave();
        }}
        disabled={isSaving}
      >
        {isSaving ? <Loader2 className="animate-spin" /> : <Save />}
      </Button>

      <Button
        variant="ghost"
        size="icon"
        title="Copy Summary"
        aria-label="Copy Summary"
        onClick={() => {
          Analytics.trackButtonClick('copy_summary', 'meeting_details');
          onCopy();
        }}
        disabled={!hasSummary}
        className={cn(headerTileClass, 'cursor-pointer')}
      >
        <Copy />
      </Button>

      {onDelete && (
        <Button
          variant="ghost"
          size="icon"
          title="Delete Meeting"
          aria-label="Delete Meeting"
          onClick={() => {
            Analytics.trackButtonClick('delete_meeting', 'meeting_details');
            onDelete();
          }}
          className={cn(headerTileClass, 'cursor-pointer hover:bg-destructive/15 hover:text-destructive')}
        >
          <Trash2 className="w-4.5 h-4.5 text-muted-foreground hover:text-destructive" />
        </Button>
      )}
    </div>
  );
}
