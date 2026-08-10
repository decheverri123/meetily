/**
 * Exact rejection reasons the backend uses for its two retry-friendly error
 * cases, shared by every frontend caller of `generate_live_insights` and
 * `generate_live_action_chip`. Must stay byte-identical to
 * `LIVE_INSIGHTS_IN_PROGRESS_ERROR` / `LIVE_INSIGHTS_RATE_LIMITED_ERROR` in
 * frontend/src-tauri/src/audio/recording_commands.rs.
 */
export const LIVE_GENERATION_IN_PROGRESS_ERROR = 'insights generation already in progress';
export const LIVE_GENERATION_RATE_LIMITED_ERROR =
  'insights generation requested too soon - please retry shortly';
