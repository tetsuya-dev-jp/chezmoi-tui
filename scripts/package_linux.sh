#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="$root/dist"
nfpm="${NFPM:-nfpm}"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1)"

if (( $# )); then
  platforms=("$@")
else
  platforms=(linux-x86_64-musl linux-aarch64-musl)
fi

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$dist"

for platform in "${platforms[@]}"; do
  case "$platform" in
    linux-x86_64-musl)
      package_arch=amd64
      file_arch=x86_64
      ;;
    linux-aarch64-musl)
      package_arch=arm64
      file_arch=aarch64
      ;;
    *)
      echo "Unsupported platform: $platform" >&2
      exit 2
      ;;
  esac

  archive="$dist/chezmoi-tui_${version}_${platform}.tar.gz"
  package_root="$staging/$platform"
  mkdir -p "$package_root"
  tar -xzf "$archive" -C "$package_root"
  for file in chezmoi-tui LICENSE README.md; do
    test -f "$package_root/$file"
  done
  file "$package_root/chezmoi-tui" | grep -Eq "statically linked|static-pie linked"

  for item in \
    "deb:deb" \
    "rpm:rpm" \
    "archlinux:pkg.tar.zst" \
    "apk:apk"; do
    packager="${item%%:*}"
    extension="${item#*:}"
    target_arch="$file_arch"
    if [[ "$packager" == deb ]]; then
      target_arch="$package_arch"
    fi
    target="$dist/chezmoi-tui_${version}_${target_arch}.${extension}"
    PACKAGE_ARCH="$package_arch" \
      PACKAGE_VERSION="$version" \
      PACKAGE_ROOT="$package_root" \
      "$nfpm" package --config "$root/packaging/nfpm.yaml" \
        --packager "$packager" --target "$target"
  done
done
