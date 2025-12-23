#!/bin/bash
set -e

# Install dependencies (if not already installed)
echo "Ensuring Runtime and SDK are installed..."
# We use --or-update to ensure we have the latest version
flatpak install --user --noninteractive --or-update org.gnome.Platform//49 org.gnome.Sdk//49 org.freedesktop.Sdk.Extension.rust-stable//25.08

# Build the Flatpak
echo "Building Flatpak..."
# --force-clean ensures a fresh build
# --install installs it to the user's flatpak installation
flatpak-builder --user --install --force-clean build-dir org.example.TodosExtension.yml

echo "Build complete! You can run the app with:"
echo "flatpak run org.example.TodosExtension"
