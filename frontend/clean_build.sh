#!/bin/bash
set -e

# Clean up extended attributes and detritus for macOS code signing
if [[ "$OSTYPE" == "darwin"* ]]; then
    xattr -cr src-tauri 2>/dev/null || true
    find src-tauri -name ".DS_Store" -delete 2>/dev/null || true
    find src-tauri -name "._*" -delete 2>/dev/null || true
fi

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

# Clean up previous builds safely
echo "Cleaning up previous builds..."
(cd .. && cargo clean 2>/dev/null) || true
rm -rf target ../target src-tauri/target src-tauri/gen 2>/dev/null || true

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
pkill -x meetily 2>/dev/null || pkill -f meetily.app 2>/dev/null || true

POSSIBLE_PATHS=(
    "../target/release/bundle/macos/meetily.app"
    "../target/release/bundle/macos/Meetily.app"
    "target/release/bundle/macos/meetily.app"
    "target/release/bundle/macos/Meetily.app"
    "src-tauri/target/release/bundle/macos/meetily.app"
    "src-tauri/target/release/bundle/macos/Meetily.app"
)

SOURCE_APP=""
for path in "${POSSIBLE_PATHS[@]}"; do
    if [ -d "$path" ]; then
        SOURCE_APP="$path"
        break
    fi
done

if [ -n "$SOURCE_APP" ] && [ -d "$SOURCE_APP" ]; then
    echo "Found built app at $SOURCE_APP"

    for TARGET_DIR in "$HOME/Applications" "/Applications"; do
        if [ -d "$TARGET_DIR" ]; then
            TARGET_APP="$TARGET_DIR/meetily.app"
            rm -rf "$TARGET_APP" 2>/dev/null || true
            rm -rf "$TARGET_DIR/Meetily.app" 2>/dev/null || true
            echo "Copying app to $TARGET_APP..."
            cp -R "$SOURCE_APP" "$TARGET_APP" 2>/dev/null && echo "✓ App deployed to $TARGET_APP" || true
        fi
    done
    echo "✓ App deployment completed"
else
    echo "✓ App deployment completed (installed via tauri-auto)"
fi


