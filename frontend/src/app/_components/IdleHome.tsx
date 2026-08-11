'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useRouter } from 'next/navigation';
import { Mic, Search, ShieldAlert, ShieldCheck, SlidersHorizontal, Upload } from 'lucide-react';
import { toast } from 'sonner';
import { PermissionWarning } from '@/components/PermissionWarning';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { ModalType } from '@/hooks/useModalState';
import { cn } from '@/lib/utils';

const SEARCH_DEBOUNCE_MS = 250;

const SECONDARY_ACTION_CLASS =
  'glass-card flex items-center gap-2 px-6 py-3 text-foreground/85 transition-colors hover:bg-secondary/10';

// Recordings are auto-titled `Meeting DD_MM_YY_HH_MM_SS` by useRecordingStart.
// api_get_meetings only returns { id, title }, so the title is the one place a
// recording date is actually available - meetings the user renamed simply get
// no date line rather than a made-up one.
const AUTO_TITLE_PATTERN = /^Meeting (\d{2})_(\d{2})_(\d{2})_(\d{2})_(\d{2})_(\d{2})$/;

function parseRecordedAt(title: string): Date | null {
  const match = AUTO_TITLE_PATTERN.exec(title.trim());
  if (!match) return null;
  const [, day, month, year, hour, minute, second] = match;
  const date = new Date(2000 + Number(year), Number(month) - 1, Number(day), Number(hour), Number(minute), Number(second));
  return Number.isNaN(date.getTime()) ? null : date;
}

function formatRecordedAt(date: Date): string {
  const midnightOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const daysAgo = Math.round((midnightOf(new Date()) - midnightOf(date)) / 86_400_000);
  const time = date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });

  if (daysAgo === 0) return `Today · ${time}`;
  if (daysAgo === 1) return `Yesterday · ${time}`;
  return `${date.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })} · ${time}`;
}

interface DeviceStatusRowProps {
  label: string;
  deviceName: string | null;
  /** null while the device probe is still in flight. */
  isAvailable: boolean | null;
}

function DeviceStatusRow({ label, deviceName, isAvailable }: DeviceStatusRowProps) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline gap-3 font-mono text-[11px] tracking-[0.12em] text-muted-foreground">
        <span>{label}</span>
        <span className={cn('ml-auto truncate text-right', isAvailable === false ? 'text-destructive' : 'text-foreground/75')}>
          {isAvailable === null ? 'Checking…' : isAvailable ? (deviceName ?? 'System default') : 'Not detected'}
        </span>
      </div>
      {/* Binary availability, not a live level meter - there is no audio level to
          read while idle, so the bar is either full (device present) or empty. */}
      <div className="h-[5px] overflow-hidden rounded-[3px] bg-secondary/10">
        {isAvailable === true && <div className="h-full w-full rounded-[3px] bg-gradient-to-r from-success to-primary" />}
      </div>
    </div>
  );
}

interface MeetingCardProps {
  id: string;
  title: string;
  preview?: string;
  onOpen: (id: string, title: string) => void;
}

function MeetingCard({ id, title, preview, onOpen }: MeetingCardProps) {
  const recordedAt = parseRecordedAt(title);

  return (
    <button
      type="button"
      onClick={() => onOpen(id, title)}
      className="glass-card flex flex-col gap-3 p-[22px] text-left transition-colors hover:bg-secondary/[.09] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
    >
      {recordedAt && (
        <div className="font-mono text-[10.5px] uppercase tracking-[0.12em] text-muted-foreground">
          {formatRecordedAt(recordedAt)}
        </div>
      )}
      <div className="text-base font-semibold text-foreground">{title}</div>
      {preview && <p className="line-clamp-1 text-[13px] text-muted-foreground">{preview}</p>}
    </button>
  );
}

interface IdleHomeProps {
  onStartRecording: () => Promise<void>;
  showModal: (name: ModalType, message?: string) => void;
  hasMicrophone: boolean;
  hasSystemAudio: boolean;
  isCheckingPermissions: boolean;
  onRecheckPermissions: () => void;
}

/**
 * Home screen shown while nothing is being recorded. The recent-meetings grid
 * exists so opening a meeting is a single click, instead of expand-sidebar ->
 * find-in-tree -> click.
 */
export function IdleHome({
  onStartRecording,
  showModal,
  hasMicrophone,
  hasSystemAudio,
  isCheckingPermissions,
  onRecheckPermissions,
}: IdleHomeProps) {
  const router = useRouter();
  const { meetings, setCurrentMeeting, searchTranscripts, searchResults } = useSidebar();
  const { selectedDevices } = useConfig();
  const { openImportDialog } = useImportDialog();

  const [query, setQuery] = useState('');
  const searchInputRef = useRef<HTMLInputElement>(null);

  // usePermissionCheck reports "no devices" until its first probe resolves, so
  // wait for that before claiming anything is missing. Sticky, so a manual
  // recheck doesn't flip the panel back to "Checking…".
  const hasProbedDevicesRef = useRef(false);
  if (!isCheckingPermissions) hasProbedDevicesRef.current = true;
  const hasProbedDevices = hasProbedDevicesRef.current;

  // searchTranscripts is recreated on every SidebarProvider render, so hold it
  // in a ref to keep it out of the debounce effect's dependency list.
  const searchTranscriptsRef = useRef(searchTranscripts);
  searchTranscriptsRef.current = searchTranscripts;

  useEffect(() => {
    const timer = setTimeout(() => searchTranscriptsRef.current(query), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        searchInputRef.current?.focus();
      }
    };
    window.addEventListener('keydown', focusSearch);
    return () => window.removeEventListener('keydown', focusSearch);
  }, []);

  const openMeeting = useCallback((id: string, title: string) => {
    setCurrentMeeting({ id, title });
    router.push(`/meeting-details?id=${id}`);
  }, [router, setCurrentMeeting]);

  const handleStartRecording = useCallback(async () => {
    try {
      await onStartRecording();
    } catch (error) {
      // handleRecordingStart re-throws so its RecordingControls caller can render
      // device-specific errors; that control is hidden here, so surface it instead.
      toast.error('Failed to start recording', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  }, [onStartRecording]);

  // Transcript hits from api_search_transcripts, keyed by meeting so a matching
  // snippet can be shown as the card preview.
  const transcriptMatches = useMemo(() => {
    const matches = new Map<string, string>();
    if (!query.trim()) return matches;
    for (const result of searchResults) {
      if (result.matchContext) matches.set(result.id, result.matchContext);
    }
    return matches;
  }, [searchResults, query]);

  const visibleMeetings = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return meetings;
    return meetings.filter(m => m.title.toLowerCase().includes(needle) || transcriptMatches.has(m.id));
  }, [meetings, query, transcriptMatches]);

  return (
    <div className="flex flex-1 flex-col gap-[22px] overflow-y-auto p-8">
      <section className="glass-panel flex flex-none flex-col gap-11 rounded-3xl px-11 py-10 lg:flex-row">
        <div className="flex flex-1 flex-col gap-4">
          <p className="font-mono text-[11px] uppercase tracking-[0.12em] text-muted-foreground">
            Ready · Nothing leaves this machine
          </p>
          <h1 className="text-[40px] font-semibold leading-tight tracking-[-0.03em] text-foreground">
            Start a recording
          </h1>
          <p className="max-w-[52ch] text-[15.5px] leading-relaxed text-muted-foreground">
            Your microphone and system audio are mixed locally, transcribed on device, and
            summarised by your own model. Nothing is uploaded.
          </p>

          <div className="mt-2 flex flex-wrap items-center gap-3">
            <button
              type="button"
              onClick={handleStartRecording}
              className="flex items-center gap-2 rounded-2xl bg-destructive px-6 py-3 font-semibold text-white shadow-[0_10px_30px_-8px_hsl(var(--destructive)/0.7)] transition-colors hover:bg-destructive/90"
            >
              <Mic className="h-4 w-4" />
              Record now
            </button>
            <button
              type="button"
              onClick={() => openImportDialog()}
              className={SECONDARY_ACTION_CLASS}
            >
              <Upload className="h-4 w-4" />
              Import audio
            </button>
            <button
              type="button"
              onClick={() => showModal('deviceSettings')}
              className={SECONDARY_ACTION_CLASS}
            >
              <SlidersHorizontal className="h-4 w-4" />
              Devices
            </button>
          </div>

          {hasProbedDevices && (
            <PermissionWarning
              hasMicrophone={hasMicrophone}
              hasSystemAudio={hasSystemAudio}
              onRecheck={onRecheckPermissions}
              isRechecking={isCheckingPermissions}
            />
          )}
        </div>

        <div className="flex w-full flex-none flex-col gap-4 rounded-2xl border border-border/10 bg-black/25 p-[22px] lg:w-[300px]">
          <DeviceStatusRow
            label="MIC"
            deviceName={selectedDevices.micDevice}
            isAvailable={hasProbedDevices ? hasMicrophone : null}
          />
          <DeviceStatusRow
            label="SYSTEM"
            deviceName={selectedDevices.systemDevice}
            isAvailable={hasProbedDevices ? hasSystemAudio : null}
          />
          <div className="border-t border-border/10" />
          <div className="flex items-center gap-2 font-mono text-[11px] tracking-[0.12em] text-muted-foreground">
            {!hasProbedDevices ? (
              <span>CHECKING PERMISSIONS…</span>
            ) : hasMicrophone && hasSystemAudio ? (
              <>
                <ShieldCheck className="h-3.5 w-3.5 text-success" />
                <span>PERMISSIONS GRANTED</span>
              </>
            ) : (
              <>
                <ShieldAlert className="h-3.5 w-3.5 text-destructive" />
                <span>PERMISSIONS NEEDED</span>
              </>
            )}
          </div>
        </div>
      </section>

      <section className="flex min-h-0 flex-1 flex-col gap-4">
        <div className="flex flex-wrap items-center gap-3">
          <h2 className="text-sm font-semibold text-foreground">Recent meetings</h2>
          <div className="ml-auto flex min-w-[280px] items-center gap-2 rounded-xl border border-border/10 bg-secondary/5 px-3.5 py-2">
            <Search className="h-3.5 w-3.5 flex-none text-muted-foreground" />
            <input
              ref={searchInputRef}
              value={query}
              onChange={event => setQuery(event.target.value)}
              placeholder="Search inside every transcript…"
              className="min-w-0 flex-1 bg-transparent text-[13px] text-foreground outline-none placeholder:text-muted-foreground"
            />
            <span className="flex-none rounded-md border border-border/10 bg-secondary/5 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
              ⌘K
            </span>
          </div>
        </div>

        {visibleMeetings.length > 0 ? (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {visibleMeetings.map(meeting => (
              <MeetingCard
                key={meeting.id}
                id={meeting.id}
                title={meeting.title}
                preview={transcriptMatches.get(meeting.id)}
                onOpen={openMeeting}
              />
            ))}
          </div>
        ) : (
          <div className="glass-dashed flex flex-col items-center justify-center gap-1 px-6 py-14 text-center">
            <p className="text-sm text-muted-foreground">
              {query.trim() ? 'No meetings match that search.' : 'No meetings yet.'}
            </p>
            <p className="text-[13px] text-muted-foreground/70">
              {query.trim() ? 'Try a different word from the conversation.' : 'Hit Record now and your first meeting will show up here.'}
            </p>
          </div>
        )}
      </section>
    </div>
  );
}
