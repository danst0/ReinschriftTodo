#!/bin/bash
set -e

# Install dependencies (if not already installed)
echo "Ensuring Runtime and SDK are installed..."
# We use --or-update to ensure we have the latest version
flatpak install --user --noninteractive --or-update org.gnome.Platform//49 org.gnome.Sdk//49 org.freedesktop.Sdk.Extension.rust-stable//25.08 org.freedesktop.Sdk.Extension.llvm20//25.08

# Build the Flatpak
echo "Building Flatpak..."
# --force-clean ensures a fresh build
flatpak-builder --force-clean --repo=repo build-dir me.dumke.Reinschrift.yml

# Create a bundle
echo "Creating Bundle..."
flatpak build-bundle repo reinschrift.flatpak me.dumke.Reinschrift

# Install the Flatpak
echo "Installing Flatpak..."
flatpak install --user --noninteractive --or-update reinschrift.flatpak

echo "Build and install complete! You can run the app with:"
echo "flatpak run me.dumke.Reinschrift"
