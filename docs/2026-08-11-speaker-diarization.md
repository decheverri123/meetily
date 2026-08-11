# Speaker Diarization (Mic vs. System Heuristic) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tag each transcript segment with "You" (microphone) or "Them" (system audio) by attributing VAD speech segments to whichever pre-mix audio stream dominated that time range, and surface the label in the transcript UI.

**Architecture:** `AudioPipeline` (`frontend/src-tauri/src/audio/pipeline.rs`) already keeps mic and system audio as separate windows (`mic_window`, `sys_window`) in its ring buffer before mixing them for both recording and VAD/Whisper transcription. A new `speaker_attribution` module records per-window RMS energy for each stream *before* the mix, then looks up which stream dominated a completed VAD segment's `[start_timestamp_ms, end_timestamp_ms)` range. That attribution rides along on `AudioChunk` through the transcription worker, gets serialized onto the `transcript-update` event and the SQLite `transcripts.speaker` column (which already exists — see Global Constraints), and is rendered as a small badge in the transcript views.

**Tech Stack:** Rust (existing `audio` module, `anyhow::Result`, `tokio`), no new crate dependencies; SQLite via `sqlx` (existing `transcripts.speaker TEXT` column); TypeScript/React (Next.js) frontend, `bun test` + `@testing-library/react` for new frontend unit tests.

## Global Constraints

- Must stay fully local/offline — no cloud diarization API, no network calls of any kind for this feature.
- Must reuse the existing dual-stream audio pipeline (mic/system captured and buffered separately in `pipeline.rs` before mixing) rather than re-analyzing the already-mixed recording or running a second audio pass.
- Must not add a new ML model, embedding extractor, or crate dependency — attribution is pure signal-energy comparison on data the pipeline already has in memory.
- The `transcripts.speaker TEXT` column already exists (migration `frontend/src-tauri/migrations/20251110000001_add_speaker_field.sql`, values `'mic'` / `'system'`) but nothing currently writes or reads it — no new migration is needed, only wiring the column up end-to-end.
- `frontend/src-tauri/src/audio/stt.rs` contains vendored, non-compiling code (`crate::pyannote::*`, `crate::deepgram`, `crate::vad_engine`, `crate::whisper` — none of these modules exist in this crate; `stt` is not declared in `frontend/src-tauri/src/audio/mod.rs`) left over from the screenpipe project this codebase's audio capture layer was originally derived from. Do not build on it and do not wire it into `mod.rs` — it is dead reference material, not an active dependency.
- Follow existing naming convention: DB/event value `"mic"` maps to display label `"You"`, `"system"` maps to `"Them"` (matches the migration's own `'mic'`/`'system'` comment and the codebase's "microphone"/"system" device naming convention, never "input"/"output").
- Ambiguous or silent ranges (both streams quiet, or comparable energy — e.g. simultaneous talk-over, or system audio bleeding into the mic) must resolve to `None`/`null`, not a guess. The UI shows no badge in that case rather than a wrong one.

## Why not true multi-speaker clustering (v2, out of scope)

True diarization (separating multiple speakers *within* the same microphone stream, e.g. two people in the same room) needs a speaker-embedding model (e.g. pyannote-style segmentation + embedding + clustering) run locally on-device. This repo has no such model, no ONNX/embedding crate wired in for it (the only `ort` dependency in `frontend/src-tauri/Cargo.toml:106` is for the Parakeet ASR provider, not speaker embeddings), and the one place embedding-based diarization was ever wired up (`audio/stt.rs`) is dead, non-compiling code from a different, abandoned architecture. Building it properly means bundling/downloading a new local model, adding an inference dependency, and implementing online clustering — a substantial new subsystem, not a small addition. The mic-vs-system heuristic, by contrast, needs zero new dependencies and finishes a data path (the `speaker` column) the codebase already started and abandoned. v2 (in-room multi-speaker clustering) is explicitly deferred; this plan implements only the mic-vs-system heuristic (v1), fully.

---

### Task 1: Speaker attribution core logic (pure, unit-testable)

**Files:**
- Create: `frontend/src-tauri/src/audio/speaker_attribution.rs`
- Modify: `frontend/src-tauri/src/audio/mod.rs:2` (add `pub mod speaker_attribution;` alongside the other `pub mod` declarations)
- Test: inline `#[cfg(test)] mod tests` in `frontend/src-tauri/src/audio/speaker_attribution.rs`

**Interfaces:**
- Consumes: `super::recording_state::DeviceType` (`Microphone` / `System`, already `#[derive(Debug, Clone, PartialEq)]` in `frontend/src-tauri/src/audio/recording_state.rs:11-15`)
- Produces: `pub struct SourceEnergyLog` with `pub fn new() -> Self`, `pub fn record_window(&mut self, mic_window: &[f32], sys_window: &[f32], sample_rate: u32)`, `pub fn dominant_source(&self, start_ms: f64, end_ms: f64) -> Option<DeviceType>`; `pub fn classify_dominant_source(mic_energy: f32, sys_energy: f32) -> Option<DeviceType>`; `pub fn speaker_label(device_type: Option<DeviceType>) -> Option<String>` (maps `Microphone` → `"mic"`, `System` → `"system"`, `None` → `None`) — these are consumed by Task 2 (pipeline wiring) and Task 3 (worker.rs event emission).

- [ ] **Step 1: Write the failing test**

  Add to `frontend/src-tauri/src/audio/speaker_attribution.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn classify_dominant_source_picks_mic_when_clearly_louder() {
          assert_eq!(classify_dominant_source(0.5, 0.1), Some(DeviceType::Microphone));
      }

      #[test]
      fn classify_dominant_source_picks_system_when_clearly_louder() {
          assert_eq!(classify_dominant_source(0.05, 0.4), Some(DeviceType::System));
      }

      #[test]
      fn classify_dominant_source_is_none_when_comparable() {
          // Within the 1.2x dominance ratio - ambiguous, e.g. cross-talk or mic bleed.
          assert_eq!(classify_dominant_source(0.20, 0.19), None);
      }

      #[test]
      fn classify_dominant_source_is_none_when_both_silent() {
          assert_eq!(classify_dominant_source(0.0, 0.0), None);
      }

      #[test]
      fn speaker_label_maps_device_types_to_db_convention() {
          assert_eq!(speaker_label(Some(DeviceType::Microphone)), Some("mic".to_string()));
          assert_eq!(speaker_label(Some(DeviceType::System)), Some("system".to_string()));
          assert_eq!(speaker_label(None), None);
      }

      #[test]
      fn energy_log_attributes_segment_to_the_window_that_overlaps_it() {
          let mut log = SourceEnergyLog::new();
          let sample_rate = 1000u32; // 1000 samples = 1000ms, for easy arithmetic
          // Window 1: 0-1000ms, mic loud, system silent.
          log.record_window(&vec![0.8f32; 1000], &vec![0.0f32; 1000], sample_rate);
          // Window 2: 1000-2000ms, system loud, mic silent.
          log.record_window(&vec![0.0f32; 1000], &vec![0.8f32; 1000], sample_rate);

          assert_eq!(log.dominant_source(0.0, 900.0), Some(DeviceType::Microphone));
          assert_eq!(log.dominant_source(1100.0, 2000.0), Some(DeviceType::System));
      }

      #[test]
      fn energy_log_prunes_windows_older_than_max_age() {
          let mut log = SourceEnergyLog::new();
          let sample_rate = 1000u32;
          log.record_window(&vec![0.8f32; 1000], &vec![0.0f32; 1000], sample_rate);
          // Push the clock forward more than MAX_LOG_AGE_MS by recording many silent windows.
          for _ in 0..35 {
              log.record_window(&vec![0.0f32; 1000], &vec![0.0f32; 1000], sample_rate);
          }
          // The original loud mic window has aged out; no evidence remains for that range.
          assert_eq!(log.dominant_source(0.0, 900.0), None);
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::speaker_attribution -- --nocapture
  ```

  Expected: compile error (`speaker_attribution` module and its types don't exist yet / `mod.rs` doesn't declare it).

- [ ] **Step 3: Write minimal implementation**

  Register the module first — in `frontend/src-tauri/src/audio/mod.rs:2`, add `pub mod speaker_attribution;` next to the other early `pub mod` lines (e.g. right after `pub mod audio_processing;`).

  Then implement `frontend/src-tauri/src/audio/speaker_attribution.rs`:

  ```rust
  //! Mic-vs-system heuristic speaker attribution.
  //!
  //! The audio pipeline mixes mic + system audio into one stream before VAD and
  //! Whisper ever see it (see `pipeline.rs`), so a VAD segment carries no record
  //! of which pre-mix source produced it. This module tracks per-window RMS
  //! energy on both pre-mix streams as they're mixed, so a completed VAD
  //! segment's timestamp range can be retroactively attributed to whichever
  //! stream dominated it. No embeddings, no ML model - just energy comparison
  //! on data the pipeline already holds in memory.

  use std::collections::VecDeque;

  use super::recording_state::DeviceType;

  /// How much louder one source's (duration-weighted) energy must be than the
  /// other's to count as "dominant". Below this ratio the two are treated as
  /// indistinguishable (simultaneous speech, or system audio bleeding into the
  /// mic through speakers) and no attribution is made.
  const DOMINANCE_RATIO: f32 = 1.2;

  /// RMS energy floor below which a source is treated as silent.
  const SILENCE_FLOOR: f32 = 1e-6;

  /// Maximum age, in mixed-audio milliseconds, of energy samples retained by
  /// `SourceEnergyLog`. Bounds memory for long recordings; VAD segments are
  /// looked up shortly after they complete, so old windows are never queried.
  const MAX_LOG_AGE_MS: f64 = 30_000.0;

  #[derive(Debug, Clone, Copy)]
  struct EnergyWindow {
      start_ms: f64,
      end_ms: f64,
      mic_rms: f32,
      sys_rms: f32,
  }

  /// Tracks per-mixing-window mic/system RMS energy on the pre-mix streams so
  /// VAD segments produced from the *mixed* audio can be attributed back to a
  /// source after the fact.
  pub struct SourceEnergyLog {
      windows: VecDeque<EnergyWindow>,
      clock_ms: f64,
  }

  impl SourceEnergyLog {
      pub fn new() -> Self {
          Self { windows: VecDeque::new(), clock_ms: 0.0 }
      }

      /// Record one mixing window's pre-mix energy and advance the internal
      /// clock by the window's duration. Call this once per window, in the
      /// same sequential order the window is later fed into VAD, so this log's
      /// clock stays aligned with VAD's own `start_timestamp_ms`/`end_timestamp_ms`.
      pub fn record_window(&mut self, mic_window: &[f32], sys_window: &[f32], sample_rate: u32) {
          let len = mic_window.len().max(sys_window.len());
          if len == 0 || sample_rate == 0 {
              return;
          }
          let duration_ms = (len as f64 / sample_rate as f64) * 1000.0;
          let window = EnergyWindow {
              start_ms: self.clock_ms,
              end_ms: self.clock_ms + duration_ms,
              mic_rms: rms(mic_window),
              sys_rms: rms(sys_window),
          };
          self.clock_ms = window.end_ms;
          self.windows.push_back(window);
          self.prune_older_than(self.clock_ms - MAX_LOG_AGE_MS);
      }

      /// Which source dominated `[start_ms, end_ms)`, weighting each
      /// overlapping window's energy by how much of it overlaps the range.
      pub fn dominant_source(&self, start_ms: f64, end_ms: f64) -> Option<DeviceType> {
          let mut mic_energy = 0.0f64;
          let mut sys_energy = 0.0f64;
          for w in &self.windows {
              let overlap = (w.end_ms.min(end_ms) - w.start_ms.max(start_ms)).max(0.0);
              if overlap <= 0.0 {
                  continue;
              }
              mic_energy += w.mic_rms as f64 * overlap;
              sys_energy += w.sys_rms as f64 * overlap;
          }
          classify_dominant_source(mic_energy as f32, sys_energy as f32)
      }

      fn prune_older_than(&mut self, cutoff_ms: f64) {
          while let Some(front) = self.windows.front() {
              if front.end_ms < cutoff_ms {
                  self.windows.pop_front();
              } else {
                  break;
              }
          }
      }
  }

  fn rms(samples: &[f32]) -> f32 {
      if samples.is_empty() {
          return 0.0;
      }
      (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
  }

  /// Classify which source dominates, given accumulated (duration-weighted)
  /// energy for each. Returns `None` when both are near-silent or within
  /// `DOMINANCE_RATIO` of each other.
  pub fn classify_dominant_source(mic_energy: f32, sys_energy: f32) -> Option<DeviceType> {
      if mic_energy < SILENCE_FLOOR && sys_energy < SILENCE_FLOOR {
          return None;
      }
      if mic_energy > sys_energy * DOMINANCE_RATIO {
          Some(DeviceType::Microphone)
      } else if sys_energy > mic_energy * DOMINANCE_RATIO {
          Some(DeviceType::System)
      } else {
          None
      }
  }

  /// Maps a dominant source to the DB/event value convention documented in
  /// `frontend/src-tauri/migrations/20251110000001_add_speaker_field.sql`
  /// (`'mic'` for microphone, `'system'` for system audio).
  pub fn speaker_label(device_type: Option<DeviceType>) -> Option<String> {
      match device_type {
          Some(DeviceType::Microphone) => Some("mic".to_string()),
          Some(DeviceType::System) => Some("system".to_string()),
          None => None,
      }
  }
  ```

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::speaker_attribution -- --nocapture
  ```

  Expected: all 7 tests pass.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/audio/speaker_attribution.rs frontend/src-tauri/src/audio/mod.rs
  git commit -m "feat(audio): add mic-vs-system speaker attribution core logic"
  ```

---

### Task 2: Wire dominant-source attribution into the audio pipeline

**Files:**
- Modify: `frontend/src-tauri/src/audio/recording_state.rs:19-25` (add field to `AudioChunk`)
- Modify: `frontend/src-tauri/src/audio/pipeline.rs:611` (raw capture chunk — set `None`)
- Modify: `frontend/src-tauri/src/audio/pipeline.rs:681-699` (`AudioPipeline` struct — add `energy_log` field)
- Modify: `frontend/src-tauri/src/audio/pipeline.rs:701-766` (`AudioPipeline::new` — initialize `energy_log`)
- Modify: `frontend/src-tauri/src/audio/pipeline.rs:825-882` (`run()` — record energy, attribute VAD segments)
- Modify: `frontend/src-tauri/src/audio/pipeline.rs:872-880` (recording chunk — set `None`, it's mixed audio for the WAV file, not transcription)
- Modify: `frontend/src-tauri/src/audio/pipeline.rs:902-941` (`flush_remaining_audio()` — attribute final VAD segments)
- Modify: `frontend/src-tauri/src/audio/pipeline.rs:1038,1058` (flush-signal chunks — set `None`)
- Modify: `frontend/src-tauri/src/audio/incremental_saver.rs:437` (test fixture — set `None`)
- Test: new `#[cfg(test)] mod tests` block appended to `frontend/src-tauri/src/audio/pipeline.rs`

**Interfaces:**
- Consumes: `SourceEnergyLog`, `classify_dominant_source` from Task 1 (`super::speaker_attribution::SourceEnergyLog`)
- Produces: `AudioChunk.dominant_source: Option<DeviceType>`, populated only on the two transcription-chunk construction sites (VAD-segmented, mixed audio); every other `AudioChunk` construction site sets it to `None`. Consumed by Task 3 (`worker.rs`).

- [ ] **Step 1: Write the failing test**

  Append to `frontend/src-tauri/src/audio/pipeline.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use super::super::speaker_attribution::SourceEnergyLog;

      /// Exercises the same sequence Task 2's wiring performs inside
      /// `AudioPipeline::run()`: extract a mixing window from the ring buffer,
      /// record its pre-mix energy, then look up attribution for a VAD-style
      /// timestamp range. Doesn't spin up the full async pipeline - just the
      /// synchronous ring-buffer + energy-log interaction the wiring adds.
      #[test]
      fn ring_buffer_windows_feed_energy_log_in_timestamp_order() {
          let sample_rate = 48_000u32;
          let mut ring_buffer = AudioMixerRingBuffer::new(sample_rate);
          let mut energy_log = SourceEnergyLog::new();

          // Window 1: loud mic, silent system.
          let window_samples = (sample_rate as f32 * 0.6) as usize; // matches the 600ms window
          ring_buffer.add_samples(DeviceType::Microphone, vec![0.8; window_samples]);
          ring_buffer.add_samples(DeviceType::System, vec![0.0; window_samples]);
          let (mic1, sys1) = ring_buffer.extract_window().expect("window 1 should be ready");
          energy_log.record_window(&mic1, &sys1, sample_rate);

          // Window 2: silent mic, loud system.
          ring_buffer.add_samples(DeviceType::Microphone, vec![0.0; window_samples]);
          ring_buffer.add_samples(DeviceType::System, vec![0.8; window_samples]);
          let (mic2, sys2) = ring_buffer.extract_window().expect("window 2 should be ready");
          energy_log.record_window(&mic2, &sys2, sample_rate);

          assert_eq!(energy_log.dominant_source(0.0, 500.0), Some(DeviceType::Microphone));
          assert_eq!(energy_log.dominant_source(650.0, 1150.0), Some(DeviceType::System));
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::pipeline::tests -- --nocapture
  ```

  Expected: fails to compile — `AudioMixerRingBuffer` is not `pub` outside the module yet in a way the test can rely on for this exact call pattern, and `SourceEnergyLog` isn't wired in. (The test lives inside `pipeline.rs` itself via `mod tests`, so it can already see the private `AudioMixerRingBuffer`; the failure here is that the assertions don't hold until Steps 3's changes exist — run it to confirm the concrete failure before implementing.)

- [ ] **Step 3: Write minimal implementation**

  In `frontend/src-tauri/src/audio/recording_state.rs:19-25`, add the field:

  ```rust
  #[derive(Debug, Clone)]
  pub struct AudioChunk {
      pub data: Vec<f32>,
      pub sample_rate: u32,
      pub timestamp: f64,
      pub chunk_id: u64,
      pub device_type: DeviceType,
      /// Which pre-mix audio source dominated this chunk's time range, per the
      /// mic-vs-system energy heuristic (see `audio::speaker_attribution`).
      /// Only set on VAD-segmented transcription chunks built from mixed
      /// audio; `None` everywhere else (raw captures, recording chunks,
      /// flush signals) since there's nothing to attribute.
      pub dominant_source: Option<DeviceType>,
  }
  ```

  In `frontend/src-tauri/src/audio/pipeline.rs:611` (raw per-device capture chunk in `AudioCapture::process_audio_data`), add `dominant_source: None,` to the `audio_chunk` literal.

  In `frontend/src-tauri/src/audio/pipeline.rs:872-880` (`recording_chunk`, mixed audio destined for the WAV file, not Whisper), add `dominant_source: None,`.

  In `frontend/src-tauri/src/audio/pipeline.rs:1038-1044` and `:1058-1064` (`flush_chunk`, `additional_flush`), add `dominant_source: None,` to each.

  In `frontend/src-tauri/src/audio/incremental_saver.rs:437-443` (test fixture), add `dominant_source: None,`.

  Add the energy log to the pipeline struct, `frontend/src-tauri/src/audio/pipeline.rs:681-699`:

  ```rust
  pub struct AudioPipeline {
      receiver: mpsc::UnboundedReceiver<AudioChunk>,
      transcription_sender: mpsc::UnboundedSender<AudioChunk>,
      #[allow(dead_code)]
      state: Arc<RecordingState>,
      vad_processor: ContinuousVadProcessor,
      sample_rate: u32,
      chunk_id_counter: u64,
      last_summary_time: std::time::Instant,
      processed_chunks: u64,
      metrics_batcher: Option<AudioMetricsBatcher>,
      ring_buffer: AudioMixerRingBuffer,
      mixer: ProfessionalAudioMixer,
      recording_sender_for_mixed: Option<mpsc::UnboundedSender<AudioChunk>>,
      // Mic-vs-system energy tracking for speaker attribution (see speaker_attribution module).
      energy_log: super::speaker_attribution::SourceEnergyLog,
  }
  ```

  Initialize it in `AudioPipeline::new` (`frontend/src-tauri/src/audio/pipeline.rs:749-766`), inside the `Self { ... }` literal, alongside `ring_buffer` and `mixer`:

  ```rust
          energy_log: super::speaker_attribution::SourceEnergyLog::new(),
  ```

  In `run()` (`frontend/src-tauri/src/audio/pipeline.rs:825-882`), record energy right after extracting a window (before mixing consumes it) and attribute each VAD segment:

  ```rust
                      while self.ring_buffer.can_mix() {
                          if let Some((mic_window, sys_window)) = self.ring_buffer.extract_window() {
                              // Record pre-mix energy BEFORE mixing, so speaker
                              // attribution has the separate mic/system signal.
                              self.energy_log.record_window(&mic_window, &sys_window, self.sample_rate);

                              let mixed_clean = self.mixer.mix_window(&mic_window, &sys_window);
                              let mixed_with_gain = mixed_clean;

                              match self.vad_processor.process_audio(&mixed_with_gain) {
                                  Ok(speech_segments) => {
                                      for segment in speech_segments {
                                          let duration_ms = segment.end_timestamp_ms - segment.start_timestamp_ms;

                                          if segment.samples.len() >= 800 {
                                              info!("📤 Sending VAD segment: {:.1}ms, {} samples",
                                                    duration_ms, segment.samples.len());

                                              let dominant_source = self.energy_log.dominant_source(
                                                  segment.start_timestamp_ms,
                                                  segment.end_timestamp_ms,
                                              );

                                              let transcription_chunk = AudioChunk {
                                                  data: segment.samples,
                                                  sample_rate: 16000,
                                                  timestamp: segment.start_timestamp_ms / 1000.0,
                                                  chunk_id: self.chunk_id_counter,
                                                  device_type: DeviceType::Microphone,
                                                  dominant_source,
                                              };

                                              if let Err(e) = self.transcription_sender.send(transcription_chunk) {
                                                  warn!("Failed to send VAD segment: {}", e);
                                              } else {
                                                  self.chunk_id_counter += 1;
                                              }
                                          } else {
                                              debug!("⏭️ Dropping short VAD segment: {:.1}ms ({} samples < 800)",
                                                     duration_ms, segment.samples.len());
                                          }
                                      }
                                  }
                                  Err(e) => {
                                      warn!("⚠️ VAD error: {}", e);
                                  }
                              }

                              if let Some(ref sender) = self.recording_sender_for_mixed {
                                  let recording_chunk = AudioChunk {
                                      data: mixed_with_gain.clone(),
                                      sample_rate: self.sample_rate,
                                      timestamp: chunk.timestamp,
                                      chunk_id: self.chunk_id_counter,
                                      device_type: DeviceType::Microphone,
                                      dominant_source: None,
                                  };
                                  let _ = sender.send(recording_chunk);
                              }
                          }
                      }
  ```

  In `flush_remaining_audio()` (`frontend/src-tauri/src/audio/pipeline.rs:902-941`), do the same lookup for final segments:

  ```rust
                      if segment.samples.len() >= 800 {
                          info!("📤 Sending final VAD segment to Whisper: {:.1}ms duration, {} samples",
                                duration_ms, segment.samples.len());

                          let dominant_source = self.energy_log.dominant_source(
                              segment.start_timestamp_ms,
                              segment.end_timestamp_ms,
                          );

                          let transcription_chunk = AudioChunk {
                              data: segment.samples,
                              sample_rate: 16000,
                              timestamp: segment.start_timestamp_ms / 1000.0,
                              chunk_id: self.chunk_id_counter,
                              device_type: DeviceType::Microphone,
                              dominant_source,
                          };
  ```

  (`flush_remaining_audio` already takes `&mut self`, so `self.energy_log` is reachable.)

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::pipeline::tests -- --nocapture
  cd frontend/src-tauri && cargo build --lib
  ```

  Expected: the new test passes and the crate compiles (confirming every `AudioChunk` literal was updated).

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/audio/recording_state.rs frontend/src-tauri/src/audio/pipeline.rs frontend/src-tauri/src/audio/incremental_saver.rs
  git commit -m "feat(audio): attribute VAD transcription segments to mic or system audio"
  ```

---

### Task 3: Emit speaker on the `transcript-update` event

**Files:**
- Modify: `frontend/src-tauri/src/audio/transcription/worker.rs:26-39` (`TranscriptUpdate` struct)
- Modify: `frontend/src-tauri/src/audio/transcription/worker.rs:143-153` (capture `chunk.dominant_source` before the chunk moves)
- Modify: `frontend/src-tauri/src/audio/transcription/worker.rs:208-220` (populate `speaker` on the emitted `update`)
- Test: new `#[cfg(test)] mod tests` block appended to `frontend/src-tauri/src/audio/transcription/worker.rs`

**Interfaces:**
- Consumes: `AudioChunk.dominant_source` (Task 2), `speaker_attribution::speaker_label` (Task 1)
- Produces: `TranscriptUpdate.speaker: Option<String>`, serialized on the `"transcript-update"` Tauri event. Consumed by Task 5 (`recording_commands.rs` listeners) and Task 6 (frontend `TranscriptUpdate` type).

- [ ] **Step 1: Write the failing test**

  Append to `frontend/src-tauri/src/audio/transcription/worker.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      fn sample_update(speaker: Option<String>) -> TranscriptUpdate {
          TranscriptUpdate {
              text: "hello".to_string(),
              timestamp: "12:00:00".to_string(),
              source: "Audio".to_string(),
              sequence_id: 1,
              chunk_start_time: 0.0,
              is_partial: false,
              confidence: 0.9,
              audio_start_time: 0.0,
              audio_end_time: 1.0,
              duration: 1.0,
              speaker,
          }
      }

      #[test]
      fn transcript_update_serializes_speaker_when_present() {
          let json = serde_json::to_string(&sample_update(Some("mic".to_string()))).unwrap();
          assert!(json.contains("\"speaker\":\"mic\""), "expected speaker in JSON: {json}");
      }

      #[test]
      fn transcript_update_serializes_null_speaker_when_ambiguous() {
          let json = serde_json::to_string(&sample_update(None)).unwrap();
          assert!(json.contains("\"speaker\":null"), "expected null speaker in JSON: {json}");
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::transcription::worker::tests -- --nocapture
  ```

  Expected: compile error — `TranscriptUpdate` has no `speaker` field yet, so `sample_update` doesn't build.

- [ ] **Step 3: Write minimal implementation**

  In `frontend/src-tauri/src/audio/transcription/worker.rs:26-39`, add the field:

  ```rust
  #[derive(Debug, Serialize, Deserialize, Clone)]
  pub struct TranscriptUpdate {
      pub text: String,
      pub timestamp: String,
      pub source: String,
      pub sequence_id: u64,
      pub chunk_start_time: f64,
      pub is_partial: bool,
      pub confidence: f32,
      pub audio_start_time: f64,
      pub audio_end_time: f64,
      pub duration: f64,
      /// `"mic"` / `"system"` / `null` per the mic-vs-system heuristic in
      /// `audio::speaker_attribution`. `null` means the source was ambiguous
      /// (simultaneous speech, silence, or cross-talk bleed) — not "unknown".
      pub speaker: Option<String>,
  }
  ```

  In the worker loop, `frontend/src-tauri/src/audio/transcription/worker.rs:143-153`, capture the chunk's attribution before `chunk` is moved into `transcribe_chunk_with_provider`:

  ```rust
                              let chunk_timestamp = chunk.timestamp;
                              let chunk_duration = chunk.data.len() as f64 / chunk.sample_rate as f64;
                              let chunk_speaker = crate::audio::speaker_attribution::speaker_label(chunk.dominant_source.clone());
  ```

  Then in the `TranscriptUpdate` construction, `frontend/src-tauri/src/audio/transcription/worker.rs:208-220`, add the field:

  ```rust
                                          let update = TranscriptUpdate {
                                              text: transcript,
                                              timestamp: format_current_timestamp(),
                                              source: "Audio".to_string(),
                                              sequence_id,
                                              chunk_start_time: chunk_timestamp,
                                              is_partial,
                                              confidence: confidence_opt.unwrap_or(0.85),
                                              audio_start_time,
                                              audio_end_time,
                                              duration: chunk_duration,
                                              speaker: chunk_speaker.clone(),
                                          };
  ```

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::transcription::worker::tests -- --nocapture
  cd frontend/src-tauri && cargo build --lib
  ```

  Expected: both tests pass; crate compiles.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/audio/transcription/worker.rs
  git commit -m "feat(audio): include mic/system speaker attribution on transcript-update events"
  ```

---

### Task 4: Persist and read back `speaker` in SQLite

**Files:**
- Modify: `frontend/src-tauri/src/api/api.rs:183-195` (`TranscriptSegment` — DB write shape)
- Modify: `frontend/src-tauri/src/database/repositories/transcript.rs:49-61` (`save_transcript` INSERT)
- Modify: `frontend/src-tauri/src/database/models.rs:25-38` (`Transcript` — DB read shape)
- Modify: `frontend/src-tauri/src/audio/common.rs:59-67` (`create_transcript_segments` — import/retranscription path, no attribution available, sets `None`)
- Modify: `frontend/src-tauri/src/audio/import.rs:1177-1193` (test literals for the struct above)
- Test: new `#[cfg(test)] mod tests` block appended to `frontend/src-tauri/src/database/repositories/transcript.rs`

**Interfaces:**
- Consumes: `crate::database::repositories::test_support::{setup_pool, insert_meeting}` (existing helper, `frontend/src-tauri/src/database/repositories/test_support.rs`)
- Produces: `transcripts.speaker` column round-trips through `TranscriptsRepository::save_transcript` and `sqlx::query_as::<_, Transcript>`. Consumed by the frontend read path (Task 6) via `get_meeting_transcripts` / `get_meeting_transcripts_paginated`, both of which already `SELECT *` into `Transcript`.

- [ ] **Step 1: Write the failing test**

  Append to `frontend/src-tauri/src/database/repositories/transcript.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::database::models::Transcript;
      use crate::database::repositories::test_support::setup_pool;

      fn segment(text: &str, speaker: Option<&str>) -> TranscriptSegment {
          TranscriptSegment {
              id: format!("seg-{}", text),
              text: text.to_string(),
              timestamp: "00:00:00".to_string(),
              audio_start_time: Some(0.0),
              audio_end_time: Some(1.0),
              duration: Some(1.0),
              speaker: speaker.map(|s| s.to_string()),
          }
      }

      #[tokio::test]
      async fn save_transcript_round_trips_speaker_through_sqlite() {
          let pool = setup_pool().await;
          let segments = vec![
              segment("hello", Some("mic")),
              segment("hi there", Some("system")),
              segment("uh", None),
          ];

          let meeting_id = TranscriptsRepository::save_transcript(&pool, "Test Meeting", &segments, None)
              .await
              .expect("save_transcript failed");

          let rows = sqlx::query_as::<_, Transcript>(
              "SELECT * FROM transcripts WHERE meeting_id = ? ORDER BY transcript",
          )
          .bind(&meeting_id)
          .fetch_all(&pool)
          .await
          .expect("failed to read back transcripts");

          let by_text: std::collections::HashMap<_, _> =
              rows.iter().map(|r| (r.transcript.clone(), r.speaker.clone())).collect();

          assert_eq!(by_text.get("hello").unwrap(), &Some("mic".to_string()));
          assert_eq!(by_text.get("hi there").unwrap(), &Some("system".to_string()));
          assert_eq!(by_text.get("uh").unwrap(), &None);
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib database::repositories::transcript::tests -- --nocapture
  ```

  Expected: compile error — `TranscriptSegment` has no `speaker` field, `Transcript` has no `speaker` field.

- [ ] **Step 3: Write minimal implementation**

  In `frontend/src-tauri/src/api/api.rs:183-195`:

  ```rust
  #[derive(Debug, Serialize, Deserialize)]
  pub struct TranscriptSegment {
      pub id: String,
      pub text: String,
      pub timestamp: String,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub audio_start_time: Option<f64>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub audio_end_time: Option<f64>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub duration: Option<f64>,
      /// `"mic"` / `"system"` / `None` — see `audio::speaker_attribution`.
      #[serde(skip_serializing_if = "Option::is_none")]
      pub speaker: Option<String>,
  }
  ```

  In `frontend/src-tauri/src/database/repositories/transcript.rs:49-61`:

  ```rust
              let result = sqlx::query(
                  "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
              )
              .bind(&transcript_id)
              .bind(&meeting_id)
              .bind(&segment.text)
              .bind(&segment.timestamp)
              .bind(segment.audio_start_time)
              .bind(segment.audio_end_time)
              .bind(segment.duration)
              .bind(&segment.speaker)
              .execute(&mut *transaction)
              .await;
  ```

  In `frontend/src-tauri/src/database/models.rs:25-38`:

  ```rust
  #[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
  pub struct Transcript {
      pub id: String,
      pub meeting_id: String,
      pub transcript: String,
      pub timestamp: String,
      pub summary: Option<String>,
      pub action_items: Option<String>,
      pub key_points: Option<String>,
      pub audio_start_time: Option<f64>,
      pub audio_end_time: Option<f64>,
      pub duration: Option<f64>,
      /// `"mic"` / `"system"` / `None` — see `audio::speaker_attribution`.
      pub speaker: Option<String>,
  }
  ```

  Fix the two call sites that construct `api::TranscriptSegment` and would otherwise fail to compile: `frontend/src-tauri/src/audio/common.rs:59-67` (`create_transcript_segments`, used by the audio-import and retranscription flows, which only ever see the final mixed audio file — no separate mic/system streams survive to attribute, so this is always `None`):

  ```rust
              TranscriptSegment {
                  id: format!("transcript-{}", Uuid::new_v4()),
                  text: text.trim().to_string(),
                  timestamp: chrono::Utc::now().to_rfc3339(),
                  audio_start_time: Some(start_seconds),
                  audio_end_time: Some(end_seconds),
                  duration: Some(duration),
                  speaker: None,
              }
  ```

  And the test fixtures in `frontend/src-tauri/src/audio/import.rs:1177-1193` — add `speaker: None,` to both `TranscriptSegment { ... }` literals.

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib database::repositories::transcript::tests -- --nocapture
  cd frontend/src-tauri && cargo test --lib audio::common audio::import -- --nocapture
  cd frontend/src-tauri && cargo build --lib
  ```

  Expected: new test passes; existing `common`/`import` tests still pass; crate compiles.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/api/api.rs frontend/src-tauri/src/database/repositories/transcript.rs frontend/src-tauri/src/database/models.rs frontend/src-tauri/src/audio/common.rs frontend/src-tauri/src/audio/import.rs
  git commit -m "feat(db): persist and read back speaker attribution on transcripts"
  ```

---

### Task 5: Track speaker in the live in-memory transcript history

**Files:**
- Modify: `frontend/src-tauri/src/audio/recording_saver.rs:15-25` (`TranscriptSegment` — in-memory/JSON export shape)
- Modify: `frontend/src-tauri/src/audio/recording_saver.rs:122-134` (`add_transcript_chunk` legacy constructor)
- Modify: `frontend/src-tauri/src/audio/recording_commands.rs:298-307` (listener #1: build segment from `TranscriptUpdate`)
- Modify: `frontend/src-tauri/src/audio/recording_commands.rs:469-478` (listener #2: same, second registration site)
- Modify: `frontend/src-tauri/src/audio/recording_commands.rs:2204-2213` (`seg()` test helper — fix broken literal)
- Test: extend existing tests in `frontend/src-tauri/src/audio/recording_commands.rs` (`live_insights_window_tests` module) plus one new inline test

**Interfaces:**
- Consumes: `TranscriptUpdate.speaker` (Task 3)
- Produces: `recording_saver::TranscriptSegment.speaker: Option<String>`, returned by `get_transcript_history` (`frontend/src-tauri/src/audio/recording_commands.rs:1065`) and written into `transcripts.json` for a meeting folder. Consumed by Task 6 (frontend reload-sync path).

- [ ] **Step 1: Write the failing test**

  In `frontend/src-tauri/src/audio/recording_commands.rs`, inside the existing `live_insights_window_tests` module (starting at line 2188), extend the `seg` helper's callers with a new test:

  ```rust
      #[test]
      fn seg_helper_defaults_speaker_to_none() {
          // Sanity check for the `seg()` test fixture used throughout this
          // module: adding a new required field to `TranscriptSegment` must not
          // silently give every existing test a wrong non-None speaker.
          let s = seg("1", "hello");
          assert_eq!(s.speaker, None);
      }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::recording_commands -- --nocapture
  ```

  Expected: compile error — `TranscriptSegment` (in `recording_saver.rs`) has no `speaker` field, so `seg()` at line 2205 doesn't build and neither does the new assertion.

- [ ] **Step 3: Write minimal implementation**

  In `frontend/src-tauri/src/audio/recording_saver.rs:14-25`:

  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct TranscriptSegment {
      pub id: String,
      pub text: String,
      pub audio_start_time: f64,
      pub audio_end_time: f64,
      pub duration: f64,
      pub display_time: String,
      pub confidence: f32,
      pub sequence_id: u64,
      /// `"mic"` / `"system"` / `None` — see `audio::speaker_attribution`.
      pub speaker: Option<String>,
  }
  ```

  In `frontend/src-tauri/src/audio/recording_saver.rs:122-134` (`add_transcript_chunk`), add `speaker: None,` to the literal (this legacy path has no `TranscriptUpdate` to read a speaker from).

  In `frontend/src-tauri/src/audio/recording_commands.rs:298-307` (listener #1):

  ```rust
                  let segment = crate::audio::recording_saver::TranscriptSegment {
                      id: format!("seg_{}", update.sequence_id),
                      text: update.text.clone(),
                      audio_start_time: update.audio_start_time,
                      audio_end_time: update.audio_end_time,
                      duration: update.duration,
                      display_time: update.timestamp.clone(),
                      confidence: update.confidence,
                      sequence_id: update.sequence_id,
                      speaker: update.speaker.clone(),
                  };
  ```

  Apply the identical `speaker: update.speaker.clone(),` addition at the second registration site, `frontend/src-tauri/src/audio/recording_commands.rs:469-478`.

  In the `seg()` test helper, `frontend/src-tauri/src/audio/recording_commands.rs:2204-2213`, add `speaker: None,` to the literal.

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib audio::recording_commands -- --nocapture
  cd frontend/src-tauri && cargo build --lib
  ```

  Expected: the new test and every pre-existing test in `recording_commands.rs` pass; crate compiles.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/audio/recording_saver.rs frontend/src-tauri/src/audio/recording_commands.rs
  git commit -m "feat(audio): carry speaker attribution through live transcript history"
  ```

---

### Task 6: Frontend types and state plumbing

**Files:**
- Modify: `frontend/src/types/index.ts:7-19` (`Transcript`), `:21-33` (`TranscriptUpdate`), `:104-110` (`TranscriptSegmentData`)
- Modify: `frontend/src/contexts/TranscriptContext.tsx:305-318` (buffered-update construction), `:374-386` (reload-sync history mapping), `:416-427` (`addTranscript` construction)
- Create: `frontend/src/lib/transcriptSegments.ts` (pure `toTranscriptSegmentData` mapping, extracted from `TranscriptPanel.tsx`)
- Modify: `frontend/src/components/MeetingDetails/TranscriptPanel.tsx:69-82` (use the extracted mapping instead of the inline one)
- Test: `frontend/src/lib/transcriptSegments.test.ts`

**Interfaces:**
- Consumes: `speaker?: string` surfaced via `get_transcript_history` (Task 5) and the `transcript-update` event payload (Task 3)
- Produces: `Transcript.speaker`, `TranscriptSegmentData.speaker`, and `toTranscriptSegmentData(transcripts: Transcript[]): TranscriptSegmentData[]`. Consumed by Task 7 (badge rendering in both transcript views).

- [ ] **Step 1: Write the failing test**

  Create `frontend/src/lib/transcriptSegments.test.ts`:

  ```typescript
  import { describe, expect, test } from 'bun:test';
  import { toTranscriptSegmentData } from './transcriptSegments';
  import type { Transcript } from '@/types';

  function transcript(overrides: Partial<Transcript>): Transcript {
    return {
      id: 't1',
      text: 'hello',
      timestamp: '00:00:00',
      ...overrides,
    };
  }

  describe('toTranscriptSegmentData', () => {
    test('carries the speaker field through', () => {
      const result = toTranscriptSegmentData([
        transcript({ id: 'a', speaker: 'mic', audio_start_time: 1, audio_end_time: 2 }),
        transcript({ id: 'b', speaker: 'system', audio_start_time: 3, audio_end_time: 4 }),
        transcript({ id: 'c', speaker: undefined, audio_start_time: 5, audio_end_time: 6 }),
      ]);

      expect(result.map(r => r.speaker)).toEqual(['mic', 'system', undefined]);
    });

    test('defaults timestamp to 0 when audio_start_time is missing', () => {
      const result = toTranscriptSegmentData([transcript({ audio_start_time: undefined })]);
      expect(result[0].timestamp).toBe(0);
    });
  });
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend && bun test src/lib/transcriptSegments.test.ts
  ```

  Expected: fails — `./transcriptSegments` module doesn't exist yet.

- [ ] **Step 3: Write minimal implementation**

  Add `speaker?: string` to the three interfaces in `frontend/src/types/index.ts`:

  ```typescript
  export interface Transcript {
    id: string;
    text: string;
    timestamp: string;
    sequence_id?: number;
    chunk_start_time?: number;
    is_partial?: boolean;
    confidence?: number;
    audio_start_time?: number;
    audio_end_time?: number;
    duration?: number;
    /** "mic" | "system" | undefined — see Rust audio::speaker_attribution. Undefined means ambiguous, not unknown. */
    speaker?: string;
  }

  export interface TranscriptUpdate {
    text: string;
    timestamp: string;
    source: string;
    sequence_id: number;
    chunk_start_time: number;
    is_partial: boolean;
    confidence: number;
    audio_start_time: number;
    audio_end_time: number;
    duration: number;
    /** "mic" | "system" | undefined */
    speaker?: string;
  }
  ```

  And in `TranscriptSegmentData` (`frontend/src/types/index.ts:104-110`):

  ```typescript
  export interface TranscriptSegmentData {
    id: string;
    timestamp: number;
    endTime?: number;
    text: string;
    confidence?: number;
    /** "mic" | "system" | undefined */
    speaker?: string;
  }
  ```

  Create `frontend/src/lib/transcriptSegments.ts`:

  ```typescript
  import type { Transcript, TranscriptSegmentData } from '@/types';

  /**
   * Converts full Transcript records into the lighter TranscriptSegmentData
   * shape the virtualized transcript view renders. Extracted from
   * MeetingDetails/TranscriptPanel.tsx so the mapping is unit-testable
   * without mounting the panel.
   */
  export function toTranscriptSegmentData(transcripts: Transcript[]): TranscriptSegmentData[] {
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
      speaker: t.speaker,
    }));
  }
  ```

  In `frontend/src/components/MeetingDetails/TranscriptPanel.tsx:69-82`, replace the inline mapping with the extracted helper:

  ```typescript
    import { toTranscriptSegmentData } from '@/lib/transcriptSegments';

    // ...

    const convertedSegments = useMemo(() => {
      if (usePagination && segments) {
        return segments;
      }
      return toTranscriptSegmentData(transcripts);
    }, [transcripts, usePagination, segments]);
  ```

  Thread `speaker` through the three `Transcript` object literals in `frontend/src/contexts/TranscriptContext.tsx`:

  - Line 305-318 (buffered update): add `speaker: update.speaker,`
  - Line 374-386 (reload-sync history mapping, `segment: any`): add `speaker: segment.speaker,`
  - Line 416-427 (`addTranscript`): add `speaker: update.speaker,`

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend && bun test src/lib/transcriptSegments.test.ts
  cd frontend && pnpm exec tsc --noEmit
  ```

  Expected: both tests pass; no new type errors.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src/types/index.ts frontend/src/lib/transcriptSegments.ts frontend/src/lib/transcriptSegments.test.ts frontend/src/components/MeetingDetails/TranscriptPanel.tsx frontend/src/contexts/TranscriptContext.tsx
  git commit -m "feat(frontend): plumb speaker attribution through transcript state"
  ```

---

### Task 7: Speaker badge in the transcript views

**Files:**
- Create: `frontend/src/components/shared/SpeakerBadge.tsx`
- Create: `frontend/src/components/shared/SpeakerBadge.test.tsx`
- Modify: `frontend/src/components/TranscriptView.tsx:284-292` (badge next to the timestamp)
- Modify: `frontend/src/components/VirtualizedTranscriptView.tsx:68-84` (add `speaker` prop to `TranscriptSegment`), `:95-104` (render badge), and the two call sites passing props to it (`segment.speaker` — the two `<TranscriptSegment ... />` usages found around lines 330 and 387)

**Interfaces:**
- Consumes: `Transcript.speaker` / `TranscriptSegmentData.speaker` (Task 6)
- Produces: `<SpeakerBadge speaker={...} />`, a small presentational component reused by both transcript views (mirrors the existing pill pattern in `frontend/src/components/shared/CitationChip.tsx`).

- [ ] **Step 1: Write the failing test**

  Create `frontend/src/components/shared/SpeakerBadge.test.tsx`:

  ```tsx
  import { describe, expect, test } from 'bun:test';
  import { render, screen } from '@testing-library/react';
  import { SpeakerBadge } from './SpeakerBadge';

  describe('SpeakerBadge', () => {
    test('renders "You" for mic', () => {
      render(<SpeakerBadge speaker="mic" />);
      expect(screen.getByText('You')).toBeTruthy();
    });

    test('renders "Them" for system', () => {
      render(<SpeakerBadge speaker="system" />);
      expect(screen.getByText('Them')).toBeTruthy();
    });

    test('renders nothing when speaker is undefined (ambiguous)', () => {
      const { container } = render(<SpeakerBadge speaker={undefined} />);
      expect(container.firstChild).toBeNull();
    });
  });
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend && bun test src/components/shared/SpeakerBadge.test.tsx
  ```

  Expected: fails — `./SpeakerBadge` doesn't exist yet.

- [ ] **Step 3: Write minimal implementation**

  Create `frontend/src/components/shared/SpeakerBadge.tsx`:

  ```tsx
  'use client';

  import { cn } from '@/lib/utils';

  /**
   * Small pill labeling which audio source (mic vs. system) a transcript
   * segment was attributed to. Renders nothing for an ambiguous/unattributed
   * segment (simultaneous speech, silence, or pre-dating this feature) rather
   * than guessing.
   */
  export function SpeakerBadge({ speaker }: { speaker?: string }) {
    const label = speaker === 'mic' ? 'You' : speaker === 'system' ? 'Them' : null;
    if (!label) {
      return null;
    }

    return (
      <span
        className={cn(
          'rounded px-1.5 py-0.5 text-[10.5px] font-medium leading-none',
          speaker === 'mic' ? 'bg-primary/15 text-primary' : 'bg-secondary/40 text-foreground/70'
        )}
      >
        {label}
      </span>
    );
  }
  ```

  In `frontend/src/components/TranscriptView.tsx:284-292`, add the badge next to the timestamp:

  ```tsx
              <div className="flex items-start gap-2">
                <Tooltip>
                  <TooltipTrigger>
                    <span className="text-xs text-muted-foreground mt-1 flex-shrink-0 min-w-[50px] font-mono">
                      {transcript.audio_start_time !== undefined
                        ? formatRecordingTime(transcript.audio_start_time)
                        : transcript.timestamp}
                    </span>
                  </TooltipTrigger>
                  <TooltipContent>
                    {transcript.duration !== undefined && (
                      <span className="text-xs text-muted-foreground">
                        {transcript.duration.toFixed(1)}s
                        {transcript.confidence !== undefined && (
                          <ConfidenceIndicator
                            confidence={transcript.confidence}
                            showIndicator={showConfidence}
                          />
                        )}
                      </span>
                    )}
                  </TooltipContent>
                </Tooltip>
                <SpeakerBadge speaker={transcript.speaker} />
                <div className="flex-1">
  ```

  And add the import near the top of `frontend/src/components/TranscriptView.tsx`:

  ```typescript
  import { SpeakerBadge } from './shared/SpeakerBadge';
  ```

  In `frontend/src/components/VirtualizedTranscriptView.tsx:68-84`, add `speaker` to the memoized segment component's props:

  ```tsx
  const TranscriptSegment = memo(function TranscriptSegment({
      id,
      timestamp,
      text,
      confidence,
      speaker,
      isStreaming,
      showConfidence,
      isHighlighted = false,
  }: {
      id: string;
      timestamp: number;
      text: string;
      confidence?: number;
      speaker?: string;
      isStreaming: boolean;
      showConfidence: boolean;
      isHighlighted?: boolean;
  }) {
  ```

  Render it next to the timestamp, `frontend/src/components/VirtualizedTranscriptView.tsx:95-104`:

  ```tsx
              <div className="flex items-start gap-2">
                  <Tooltip>
                      <TooltipTrigger>
                          <span className={cn(
                              "text-xs mt-1 flex-shrink-0 min-w-[50px] font-mono",
                              isHighlighted ? "text-primary" : "text-muted-foreground"
                          )}>
                              {formatRecordingTime(timestamp)}
                          </span>
                      </TooltipTrigger>
                      <TooltipContent>
                          {confidence !== undefined && showConfidence && (
                              <ConfidenceIndicator confidence={confidence} showIndicator={showConfidence} />
                          )}
                      </TooltipContent>
                  </Tooltip>
                  <SpeakerBadge speaker={speaker} />
                  <div className="flex-1">
  ```

  Add the import at the top of `frontend/src/components/VirtualizedTranscriptView.tsx`:

  ```typescript
  import { SpeakerBadge } from './shared/SpeakerBadge';
  ```

  Finally, pass `speaker={segment.speaker}` at both `<TranscriptSegment ... />` call sites (the two usages around lines 330 and 387 that already pass `id={segment.id}`, `timestamp={segment.timestamp}`, `confidence={segment.confidence}`).

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend && bun test src/components/shared/SpeakerBadge.test.tsx
  cd frontend && pnpm exec tsc --noEmit
  ```

  Expected: all three `SpeakerBadge` tests pass; no new type errors.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src/components/shared/SpeakerBadge.tsx frontend/src/components/shared/SpeakerBadge.test.tsx frontend/src/components/TranscriptView.tsx frontend/src/components/VirtualizedTranscriptView.tsx
  git commit -m "feat(frontend): render mic/system speaker badge in transcript views"
  ```

---

## End-to-end verification (after all tasks)

- [ ] `cd frontend/src-tauri && cargo test --lib` — full Rust suite green.
- [ ] `cd frontend && bun test && pnpm exec tsc --noEmit` — full frontend suite green, no type errors.
- [ ] `cd frontend && ./clean_run.sh debug` — record a short meeting speaking into the mic, then playing something through system audio (or having a call), and confirm: (a) `RUST_LOG=app_lib::audio=debug` logs show `dominant_source` being set on transcription chunks, (b) the live `TranscriptView` shows "You"/"Them" badges as expected, (c) after stopping the recording, the saved meeting's `MeetingDetails` transcript panel still shows the same badges (confirms the SQLite round trip).
