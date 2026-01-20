#!/bin/bash
# Generate PWA icons from favicon.png using ImageMagick
# Usage: ./generate_icons.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE="$SCRIPT_DIR/favicon.png"
OUTPUT_DIR="$SCRIPT_DIR/icons"

# Check if ImageMagick is installed
if ! command -v convert &> /dev/null; then
    echo "Error: ImageMagick is required but not installed."
    echo "Install it with: sudo apt install imagemagick"
    exit 1
fi

# Check if source file exists
if [ ! -f "$SOURCE" ]; then
    echo "Error: Source file not found: $SOURCE"
    exit 1
fi

# Create output directory
mkdir -p "$OUTPUT_DIR"

echo "Generating PWA icons from $SOURCE..."

# Standard icon sizes
SIZES=(72 96 128 144 152 180 192 384 512)

for size in "${SIZES[@]}"; do
    output="$OUTPUT_DIR/icon-${size}.png"
    echo "  Creating ${size}x${size}..."
    convert "$SOURCE" -resize "${size}x${size}" -gravity center -background transparent -extent "${size}x${size}" "$output"
done

# Generate maskable icons with safe zone padding (10% padding on each side)
# Safe zone is 80% of the icon, so we need to add 25% padding (100/80 = 1.25)
echo "Generating maskable icons..."

for size in 192 512; do
    output="$OUTPUT_DIR/icon-maskable-${size}.png"
    # Inner content size (80% of total)
    inner_size=$((size * 80 / 100))
    echo "  Creating maskable ${size}x${size} (inner: ${inner_size}x${inner_size})..."
    convert "$SOURCE" \
        -resize "${inner_size}x${inner_size}" \
        -gravity center \
        -background "#3584e4" \
        -extent "${size}x${size}" \
        "$output"
done

echo ""
echo "Done! Icons generated in: $OUTPUT_DIR"
echo ""
echo "Generated files:"
ls -la "$OUTPUT_DIR"
