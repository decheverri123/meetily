#!/bin/bash

# Exit on error
set -e

# Add log level selector with default to INFO
LOG_LEVEL=${1:-info}

case $LOG_LEVEL in
    info|debug|trace)
        export RUST_LOG=$LOG_LEVEL
        ;;
    *)
        echo "Invalid log level: $LOG_LEVEL. Valid options: info, debug, trace"
        exit 1
        ;;
esac

# Check and install CMake if needed
echo "Checking CMake version..."
if ! command -v cmake &> /dev/null; then
    echo "CMake not found. Installing via Homebrew..."
    brew install cmake
else
    CMAKE_VERSION=$(cmake --version | head -n1 | cut -d" " -f3)
    if [[ "$CMAKE_VERSION" < "3.5" ]]; then
        echo "CMake version $CMAKE_VERSION is too old. Updating via Homebrew..."
        brew upgrade cmake
    fi
fi

# Clean up previous builds
echo "Cleaning up previous builds..."
rm -rf target/
rm -rf src-tauri/target
rm -rf src-tauri/gen

# Clean up npm, pnp and next
echo "Cleaning up npm, pnp and next..."
rm -rf node_modules
rm -rf .next
rm -rf .pnp.cjs
rm -rf out

echo "Installing dependencies..."
pnpm install

# Build the Next.js application first
echo "Building Next.js application..."
pnpm run build

# Set environment variables for the build

echo "Building Tauri app..."
./build-gpu.sh

# Deploy to Applications folder
echo "Deploying app to Applications folder..."
BUILD_DIR="src-tauri/target/release/bundle/macos"
APP_NAME="Meetily.app"
SOURCE_APP="$BUILD_DIR/$APP_NAME"
TARGET_APP="$HOME/Applications/$APP_NAME"

if [ -d "$SOURCE_APP" ]; then
    echo "Found built app at $SOURCE_APP"

    # Create Applications folder if it doesn't exist
    mkdir -p "$HOME/Applications"

    # Remove old app if it exists
    if [ -d "$TARGET_APP" ]; then
        echo "Removing old app from $TARGET_APP..."
        rm -rf "$TARGET_APP"
    fi

    # Move the new app
    echo "Moving app to $TARGET_APP..."
    mv "$SOURCE_APP" "$TARGET_APP"

    echo "✓ App deployed successfully to Applications folder"
else
    echo "✗ Build output not found at $SOURCE_APP"
    echo "Build may have failed or output location changed"
    exit 1
fi

