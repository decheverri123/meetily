-- Nullable FK from meetings to folders. ON DELETE SET NULL keeps meetings
-- (unfiled) when their folder is deleted.
ALTER TABLE meetings ADD COLUMN meeting_folder_id TEXT REFERENCES folders(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_meetings_folder_id ON meetings(meeting_folder_id);
