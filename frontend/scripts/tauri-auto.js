#!/usr/bin/env node
/**
 * Auto-detect GPU and run Tauri with appropriate features
 */

const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

// Get the command (dev or build)
const command = process.argv[2];
if (!command || !['dev', 'build'].includes(command)) {
  console.error('Usage: node tauri-auto.js [dev|build]');
  process.exit(1);
}

// Detect GPU feature
let feature = '';

// Check for environment variable override first
if (process.env.TAURI_GPU_FEATURE) {
  feature = process.env.TAURI_GPU_FEATURE;
  console.log(`🔧 Using forced GPU feature from environment: ${feature}`);
} else {
  try {
    const result = execSync('node scripts/auto-detect-gpu.js', {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'inherit']
    });
    feature = result.trim();
  } catch (err) {
    // If detection fails, continue with no features
  }
}

console.log(''); // Empty line for spacing

// Platform-specific environment variables
const platform = os.platform();
const env = { ...process.env };

if (platform === 'linux' && feature === 'cuda') {
  console.log('🐧 Linux/CUDA detected: Setting CMAKE flags for NVIDIA GPU');
  env.CMAKE_CUDA_ARCHITECTURES = '75';
  env.CMAKE_CUDA_STANDARD = '17';
  env.CMAKE_POSITION_INDEPENDENT_CODE = 'ON';
}

// Build the tauri command
let tauriCmd = `npx @tauri-apps/cli ${command}`;
if (feature && feature !== 'none') {
  tauriCmd += ` -- --features ${feature}`;
  console.log(`🚀 Running: tauri ${command} with features: ${feature}`);
} else {
  console.log(`🚀 Running: tauri ${command} (CPU-only mode)`);
}
console.log('');

// Execute the command
try {
  execSync(tauriCmd, { stdio: 'inherit', env });
} catch (err) {
  process.exit(err.status || 1);
}

// Post-build actions for macOS
if (command === 'build' && platform === 'darwin') {
  const possiblePaths = [
    path.resolve(process.cwd(), '../target/release/bundle/macos/meetily.app'),
    path.resolve(process.cwd(), 'target/release/bundle/macos/meetily.app'),
    path.resolve(__dirname, '../../target/release/bundle/macos/meetily.app'),
    path.resolve(process.cwd(), '../target/release/bundle/macos/Meetily.app'),
    path.resolve(process.cwd(), 'target/release/bundle/macos/Meetily.app'),
    path.resolve(__dirname, '../../target/release/bundle/macos/Meetily.app')
  ];

  const appPath = possiblePaths.find(p => fs.existsSync(p));

  if (appPath) {
    const appName = path.basename(appPath);
    console.log(`\n🚚 Installing ${appName}...`);

    // Terminate any running instances so the app is reloaded cleanly
    try {
      execSync('pkill -x meetily 2>/dev/null || pkill -f meetily.app 2>/dev/null || true', { stdio: 'ignore' });
    } catch (_) {}

    const targetDirs = [
      '/Applications',
      path.join(os.homedir(), 'Applications')
    ];

    for (const targetDir of targetDirs) {
      if (!fs.existsSync(targetDir)) {
        try { fs.mkdirSync(targetDir, { recursive: true }); } catch (_) {}
      }
      const destPath = path.join(targetDir, appName);
      try {
        fs.rmSync(path.join(targetDir, 'meetily.app'), { recursive: true, force: true });
        fs.rmSync(path.join(targetDir, 'Meetily.app'), { recursive: true, force: true });
        execSync(`cp -R "${appPath}" "${targetDir}/"`, { stdio: 'ignore' });
        console.log(`✅ Successfully updated ${destPath}`);
      } catch (copyErr) {
        console.warn(`⚠️ Could not update ${destPath}:`, copyErr.message);
      }
    }
    console.log('');
  } else {
    console.warn('\n⚠️ Could not find built meetily.app bundle to install to Applications');
    process.exit(1);
  }
}

