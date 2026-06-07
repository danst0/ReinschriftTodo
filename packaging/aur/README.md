# AUR-Paket

PKGBUILD für das [AUR](https://aur.archlinux.org/) (`reinschrift`).

Binär-Namen wie im Flatpak: `reinschrift` (GUI), `reinschrift-cli` (CLI).

## Erstveröffentlichung

```bash
# AUR-Account + SSH-Key unter https://aur.archlinux.org einrichten, dann:
git clone ssh://aur@aur.archlinux.org/reinschrift.git aur-reinschrift
cp PKGBUILD .SRCINFO aur-reinschrift/
cd aur-reinschrift
git add PKGBUILD .SRCINFO
git commit -m "Initial release v0.23.8"
git push
```

## Neue Version veröffentlichen

```bash
# 1. pkgver in PKGBUILD anpassen, pkgrel auf 1 zurücksetzen
# 2. Checksum aktualisieren:
updpkgsums            # oder: sha256sum des neuen Tarballs manuell eintragen
# 3. .SRCINFO neu generieren:
makepkg --printsrcinfo > .SRCINFO
# 4. Lokal testen:
makepkg -si
# 5. Committen und pushen (im AUR-Klon)
```

Auf Nicht-Arch-Systemen lässt sich der Build in einem Container testen:

```bash
podman run --rm -v "$PWD:/pkg" -w /pkg archlinux:latest bash -c '
  pacman -Syu --noconfirm base-devel rust cmake clang gtk4 libadwaita alsa-lib &&
  useradd -m build && chown -R build /pkg &&
  su build -c "makepkg --noconfirm"'
```
