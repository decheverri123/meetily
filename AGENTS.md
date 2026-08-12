# AGENTS.md

## Project

**Meetily** is a privacy-first, fully-local AI meeting assistant: a Tauri 2 desktop app (Rust core + Next.js 14 frontend). All features live in `frontend/` — never touch `backend/` (archived Python/FastAPI, do not add endpoints there).

## Layout & boundaries

- `frontend/` — the app: Next.js UI in `src/`, Rust core in `frontend/src-tauri/src/` (crate `app_lib`). Tauri commands/events are the entire Rust↔frontend API surface.
- `backend/` — dead legacy archive. Ignore except as historical context.
- Root `Cargo.toml` is a cargo workspace (`frontend/src-tauri`, `llama-helper`). Run cURL-style `cargo check`/`test` from `frontend/src-tauri` or the workspace root.

## Commands (run from `frontend/`; package manager is pnpm)

- `pnpm run dev` — Next.js dev server on **port 3118** (matches `tauri.conf.json` `devUrl`; not 3000).
- `pnpm run tauri:dev` / `pnpm run tauri:build` — full desktop dev/build. Both route through `scripts/tauri-auto.js`, which auto-detects GPU and appends `--features <gpu>` to cargo. Override with `TAURI_GPU_FEATURE=metal|cuda|vulkan|none`.
- `./clean_run.sh [info|debug|trace]` — nukes `node_modules`/`.next`/`out`, reinstalls, rebuilds. Slow; only when a clean state is really needed. `./clean_build.sh` is the production build.
- Tests: **`bun test`** (not jest/npm test). Suites in `frontend/tests/`, using happy-dom + @testing-library/react.
- Lint: `pnpm run lint` (`next lint`). Rust checks: `cargo check` / `cargo test` from `src-tauri`.

## Rust core

- GPU acceleration (Metal+CoreML on macOS, CUDA/Vulkan elsewhere) is wired per-`target` in `frontend/src-tauri/Cargo.toml` — do not re-add flags globally.
- SQLite via sqlx; schema lives in `frontend/src-tauri/migrations/<timestamp>_*.sql`, applied at startup (`sqlx::migrate!` in `database/manager.rs`). Add a new migration file for schema changes.
- Shared async state uses `Arc<RwLock<T>>` / `Arc<AtomicBool>`; hot-path logging must use `perf_debug!()` / `perf_trace!()` macros from `lib.rs` (no-ops in release builds).
- Audio: `audio/pipeline.rs` runs two parallel paths — professional mixing (RMS ducking) for the recording vs. VAD-filtered speech for Whisper. Streams are 48kHz. Audio feature work maps to modules under `audio/` (capture, devices, recording_*, etc.).
- Whisper engine in `src/whisper_engine/`, Parakeet in `src/parakeet_engine/`, transcription orchestration under `audio/transcription/`.

## Frontend specifics

- `reactStrictMode: false` is intentional (BlockNote compatibility) — don't "fix" it.
- `next.config.js` aliases keep ProseMirror single-instanced for BlockNote/Tiptap; `prosemirror-*` are pinned via `overrides` in `frontend/package.json`. Adding a second copy of any ProseMirror package breaks editors.
- UI↔Rust only via `@tauri-apps/api` `invoke()` / `listen()`; payload types mirror Rust structs.
- Global UI state: `components/Sidebar/SidebarProvider.tsx`.

## Models & platform gotchas

- Whisper model paths: dev `frontend/models/`; prod macOS `~/Library/Application Support/Meetily/models/`; prod Windows `%APPDATA%\Meetily\models\`.
- macOS captures system audio via ScreenCaptureKit and requires a virtual device (e.g. BlackHole) plus screen-recording permission; microphone capture needs microphone permission.
- Git branches: `main` (stable), `fix/*`, `enhance/*`. (Repo-root `CLAUDE.md` is an older, more detailed dev guide; it may lag the current layout — trust code over it.)