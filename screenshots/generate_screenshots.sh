#!/bin/bash

# Languages to screenshot
LANGS=("de" "en" "es" "fr" "sv" "ja")

# Get version from Cargo.toml
VERSION=$(grep '^version =' Cargo.toml | cut -d '"' -f 2)

# Ensure screenshots directory exists
mkdir -p screenshots

echo "Building application (Version $VERSION)..."
cargo build --release

for LANG in "${LANGS[@]}"; do
    echo "Generating screenshot for: $LANG"
    
    # Run the app in the background with the specific database and language
    ./target/release/reinschrift_todo --database "screenshots/db_$LANG.md" --language "$LANG" &
    APP_PID=$!
    
    # Wait for the window to appear and render
    sleep 3
    
    # Take the screenshot using flameshot
    FILENAME="main_app_${LANG}_v${VERSION}.png"
    ABS_SCREENSHOT_PATH="$PWD/screenshots/$FILENAME"
    
    # Overwrite existing screenshot
    rm -f "$ABS_SCREENSHOT_PATH"
    
    # Use flameshot full
    flameshot full -p "$ABS_SCREENSHOT_PATH" > /dev/null 2>&1
    
    if [ $? -eq 0 ]; then
        echo "Screenshot saved to screenshots/$FILENAME"
    else
        echo "Error: Failed to take screenshot for $LANG using flameshot."
    fi
    
    # Kill the app
    kill $APP_PID
    sleep 1
done

echo "Done! Screenshots are in the 'screenshots' directory."
