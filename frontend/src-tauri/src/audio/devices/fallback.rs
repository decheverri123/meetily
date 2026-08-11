// Bluetooth device fallback strategy for stable Core Audio recording (macOS-specific)
//
// This module implements automatic fallback to built-in devices when
// Bluetooth devices are detected as system defaults on macOS. This solves:
// - Bluetooth variable sample rate issues (Core Audio may resample dynamically)
// - Inconsistent sample rates when mixing mic + system audio streams
// - ScreenCaptureKit capturing Bluetooth-processed streams with variable timing
//
// Strategy (macOS-only):
// 1. Get system default devices (mic + speaker)
// 2. Detect if EACH is Bluetooth using InputDeviceKind::detect()
// 3. For EACH Bluetooth device detected → Override to built-in MacBook device
// 4. Return final devices with detailed rationale logging
//
// Note: Bluetooth mic and speaker are checked INDEPENDENTLY - one, both, or
// neither could be Bluetooth and need override.
//
// User still hears via Bluetooth (playback uses default), but recording
// captures via stable wired path (built-in mic + ScreenCaptureKit from built-in).

use anyhow::Result;
use log::{info, warn};

use super::configuration::AudioDevice;
use super::microphone::{default_input_device, find_builtin_input_device};
use super::speakers::default_output_device;
use crate::audio::device_detection::InputDeviceKind;

/// Which microphone to open once a candidate has been classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicChoice {
    /// Open the candidate device as-is
    UseCandidate,
    /// Open the built-in microphone instead
    UseBuiltin,
}

/// Decide whether a candidate microphone should be swapped for the built-in one.
///
/// Opening a Bluetooth microphone forces macOS to renegotiate the headset from
/// A2DP to HFP, which drops playback to mono at 16-24kHz for as long as the
/// input stream is open. Recording from the built-in mic keeps the headset in
/// A2DP so the user's audio stays intact.
pub fn decide_microphone(kind: InputDeviceKind, builtin_available: bool) -> MicChoice {
    if kind.is_bluetooth() && builtin_available {
        MicChoice::UseBuiltin
    } else {
        MicChoice::UseCandidate
    }
}

/// Swap a Bluetooth microphone for the built-in one, keeping the headset in A2DP.
///
/// Returns the candidate unchanged when it is not Bluetooth, or when no built-in
/// microphone exists to fall back to.
pub fn stabilize_microphone(candidate: &AudioDevice) -> Result<AudioDevice> {
    let kind = InputDeviceKind::detect(&candidate.name, 512, 48000);
    let builtin = if kind.is_bluetooth() {
        find_builtin_input_device()?
    } else {
        None
    };

    match (decide_microphone(kind, builtin.is_some()), builtin) {
        (MicChoice::UseBuiltin, Some(builtin)) => {
            warn!("🎧 Bluetooth microphone detected: '{}'", candidate.name);
            info!("→ ✅ Overriding to built-in microphone: '{}'", builtin.name);
            info!("   Keeps the headset in A2DP so playback quality is preserved");
            Ok(builtin)
        }
        _ => {
            if kind.is_bluetooth() {
                warn!("🎧 Bluetooth microphone '{}' in use - no built-in fallback available", candidate.name);
                warn!("   Playback through this headset will drop to call quality while recording");
            } else {
                info!("✅ Using wired/built-in microphone: '{}' (device type: {:?})", candidate.name, kind);
            }
            Ok(candidate.clone())
        }
    }
}

/// Get safe recording devices with automatic Bluetooth fallback (macOS-specific)
///
/// This function intelligently selects audio devices for recording on macOS:
/// - Checks microphone: if Bluetooth → override to built-in mic
/// - Checks speaker: if Bluetooth → override to built-in speaker
/// - Each device is evaluated INDEPENDENTLY
///
/// # Rationale for Bluetooth Override
///
/// Bluetooth devices on macOS can have variable sample rates as Core Audio
/// and the Bluetooth stack may resample dynamically. When ScreenCaptureKit
/// captures from a Bluetooth output device, it captures the processed stream
/// which may have inconsistent sample rates, causing sync issues when mixing
/// with the microphone stream.
///
/// Built-in devices have fixed, consistent sample rates → reliable mixing.
///
/// # Returns
///
/// Tuple of (microphone, system_audio) where:
/// - Some(device) = Device found and safe for recording
/// - None = No device available (non-fatal, recording can continue with single source)
///
/// # Example
///
/// ```rust
/// // When AirPods are default mic, built-in speaker is default output:
/// let (mic, system) = get_safe_recording_devices_macos()?;
///
/// // Logs:
/// // "🎧 Bluetooth microphone detected: AirPods Pro"
/// // "→ Overriding to stable built-in: MacBook Pro Microphone"
/// // "✅ Using wired speaker: MacBook Pro Speakers"
/// ```
#[cfg(target_os = "macos")]
pub fn get_safe_recording_devices_macos() -> Result<(Option<AudioDevice>, Option<AudioDevice>)> {
    info!("🔍 [macOS] Selecting recording devices with Bluetooth detection...");

    // Step 1: Get system defaults
    let default_mic = default_input_device().ok();
    let default_speaker = default_output_device().ok();

    // Step 2: Process microphone with Bluetooth override
    let final_mic = match default_mic {
        Some(mic) => Some(stabilize_microphone(&mic)?),
        None => {
            warn!("⚠️ No default microphone found");
            None
        }
    };

    // Step 3: Process speaker/system audio - KEEP AS-IS (macOS-specific behavior)
    // CRITICAL: On macOS, ScreenCaptureKit captures the digital audio stream being
    // sent to the output device BEFORE Bluetooth encoding happens. This means:
    // - If user has Bluetooth AirPods, audio is actively playing through them
    // - ScreenCaptureKit captures from that active output stream (pristine quality)
    // - We MUST keep the Bluetooth speaker as the system device so ScreenCaptureKit
    //   captures from where the audio is actually going
    //
    // If we override to built-in speakers when user is playing through Bluetooth,
    // ScreenCaptureKit will try to capture from built-in, but NO AUDIO IS THERE!
    let final_speaker = if let Some(ref speaker) = default_speaker {
        let device_kind = InputDeviceKind::detect(&speaker.name, 512, 48000);

        if device_kind.is_bluetooth() {
            warn!("🔊 Bluetooth speaker detected: '{}'", speaker.name);
            info!("   macOS: ScreenCaptureKit captures digital stream BEFORE Bluetooth encoding");
            info!("   Keeping Bluetooth speaker - captures from active output (pristine quality)");
            Some(speaker.clone())
        } else {
            info!("✅ Using wired/built-in speaker: '{}' (device type: {:?})", speaker.name, device_kind);
            Some(speaker.clone())
        }
    } else {
        warn!("⚠️ No default speaker found - system audio will not be recorded");
        None
    };

    // Summary logging
    match (&final_mic, &final_speaker) {
        (Some(mic), Some(speaker)) => {
            info!("📋 [macOS] Recording device selection complete:");
            info!("   Microphone: '{}'", mic.name);
            info!("   System Audio: '{}' (via ScreenCaptureKit)", speaker.name);
        }
        (Some(mic), None) => {
            info!("📋 [macOS] Recording device selection complete:");
            info!("   Microphone: '{}' (system audio unavailable)", mic.name);
        }
        (None, Some(speaker)) => {
            warn!("📋 [macOS] Recording device selection complete:");
            warn!("   System Audio: '{}' (microphone unavailable)", speaker.name);
        }
        (None, None) => {
            warn!("❌ No recording devices available - cannot start recording");
        }
    }

    Ok((final_mic, final_speaker))
}

// Non-macOS platforms: Just use system defaults (no Bluetooth override needed)
#[cfg(not(target_os = "macos"))]
pub fn get_safe_recording_devices() -> Result<(Option<AudioDevice>, Option<AudioDevice>)> {
    info!("🔍 Selecting default recording devices (no Bluetooth override on this platform)");

    let mic = default_input_device().ok();
    let speaker = default_output_device().ok();

    Ok((mic, speaker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_mic_is_swapped_for_builtin_when_one_exists() {
        assert_eq!(
            decide_microphone(InputDeviceKind::Bluetooth, true),
            MicChoice::UseBuiltin
        );
    }

    #[test]
    fn bluetooth_mic_is_kept_when_no_builtin_exists() {
        assert_eq!(
            decide_microphone(InputDeviceKind::Bluetooth, false),
            MicChoice::UseCandidate
        );
    }

    #[test]
    fn wired_mic_is_never_swapped() {
        assert_eq!(
            decide_microphone(InputDeviceKind::Wired, true),
            MicChoice::UseCandidate
        );
    }

    #[test]
    fn unknown_mic_is_never_swapped() {
        assert_eq!(
            decide_microphone(InputDeviceKind::Unknown, true),
            MicChoice::UseCandidate
        );
    }
}
