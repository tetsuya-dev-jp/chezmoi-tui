#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="$root/dist"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1)"

if (( $# )); then
  platforms=("$@")
else
  platforms=(linux-x86_64-musl linux-aarch64-musl)
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/chezmoi-tui-extra.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$dist" "$work/tools"

case "$(uname -m)" in
  x86_64 | amd64)
    rattler_asset="rattler-build-x86_64-unknown-linux-musl"
    ;;
  aarch64 | arm64)
    rattler_asset="rattler-build-aarch64-unknown-linux-musl"
    ;;
  *)
    echo "Unsupported build host: $(uname -m)" >&2
    exit 2
    ;;
esac

rattler="${RATTLER_BUILD:-}"
if [[ -z "$rattler" ]]; then
  rattler="$work/tools/rattler-build"
  curl --fail --location --silent --show-error \
    "https://github.com/prefix-dev/rattler-build/releases/download/v0.79.0/$rattler_asset" \
    --output "$rattler"
  chmod +x "$rattler"
fi
test "$("$rattler" --version)" = "rattler-build 0.79.0"

for platform in "${platforms[@]}"; do
  case "$platform" in
    linux-x86_64-musl)
      conda_platform=linux-64
      snap_arch=amd64
      expected_file='x86-64'
      ;;
    linux-aarch64-musl)
      conda_platform=linux-aarch64
      snap_arch=arm64
      expected_file='aarch64'
      ;;
    *)
      echo "Unsupported platform: $platform" >&2
      exit 2
      ;;
  esac

  archive="$dist/chezmoi-tui_${version}_${platform}.tar.gz"
  test -f "$archive"
  source_dir="$work/source-$platform"
  mkdir -p "$source_dir"
  tar -xzf "$archive" -C "$source_dir"
  for file in chezmoi-tui LICENSE README.md; do
    test -f "$source_dir/$file"
  done
  file "$source_dir/chezmoi-tui" | grep -Eq "statically linked|static-pie linked"
  file "$source_dir/chezmoi-tui" | grep -q "$expected_file"

  conda_recipe="$work/conda-$platform"
  mkdir -p "$conda_recipe/source"
  sed "s/@VERSION@/$version/g" "$root/packaging/conda/recipe.yaml" \
    > "$conda_recipe/recipe.yaml"
  cp "$source_dir/chezmoi-tui" "$source_dir/LICENSE" "$source_dir/README.md" "$conda_recipe/source/"
  conda_output="$work/conda-output-$platform"
  "$rattler" build \
    --recipe "$conda_recipe/recipe.yaml" \
    --target-platform "$conda_platform" \
    --output-dir "$conda_output"
  mapfile -t conda_packages < <(find "$conda_output" -type f -name 'chezmoi-tui-*.conda')
  test "${#conda_packages[@]}" -eq 1
  conda_package="$dist/$(basename "${conda_packages[0]}")"
  cp "${conda_packages[0]}" "$conda_package"

  snap_root="$work/snap-$platform"
  mkdir -p "$snap_root/bin" "$snap_root/meta" \
    "$snap_root/share/doc/chezmoi-tui" "$snap_root/share/licenses/chezmoi-tui"
  cp "$source_dir/chezmoi-tui" "$snap_root/bin/chezmoi-tui"
  cp "$source_dir/README.md" "$snap_root/share/doc/chezmoi-tui/README.md"
  cp "$source_dir/LICENSE" "$snap_root/share/licenses/chezmoi-tui/LICENSE"
  sed -e "s/@VERSION@/$version/g" -e "s/@SNAP_ARCH@/$snap_arch/" \
    "$root/packaging/snap/snap.yaml.in" \
    > "$snap_root/meta/snap.yaml"

  snap_file="$dist/chezmoi-tui_${version}_${snap_arch}.snap"
  if command -v snap >/dev/null; then
    snap pack --filename "$(basename "$snap_file")" "$snap_root" "$dist"
  elif command -v snapcraft >/dev/null; then
    snapcraft pack "$snap_root" --output "$snap_file"
  else
    echo "Neither snap nor snapcraft is available" >&2
    exit 2
  fi
  test -f "$snap_file"
done
