#!/usr/bin/env bash

export REPO=https://github.com/tetsuya-dev-jp/chezmoi-tui

fail() {
  echo "chezmoi-tui: $*" >&2
  exit 1
}

validate_version() {
  [[ "${ASDF_INSTALL_TYPE:-}" == "version" ]] ||
    fail "only ASDF_INSTALL_TYPE=version is supported"
  [[ "${ASDF_INSTALL_VERSION:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    fail "ASDF_INSTALL_VERSION must be a stable MAJOR.MINOR.PATCH version"
}

version_at_least_0_3_1() {
  local major minor patch
  IFS=. read -r major minor patch <<< "$ASDF_INSTALL_VERSION"
  ((major > 0 || minor > 3 || (minor == 3 && patch >= 1)))
}

using_musl() {
  local output
  output="$(ldd --version 2>&1 || true)"
  case "$output" in
    *musl*) return 0 ;;
    *GNU* | *GLIBC* | *glibc*) return 1 ;;
  esac
  compgen -G '/lib/ld-musl-*.so.1' > /dev/null
}

resolve_asset_platform() {
  local arch

  case "$(uname -s)" in
    Darwin) asset_platform=macos ;;
    Linux) asset_platform=linux ;;
    *) fail "unsupported operating system: $(uname -s)" ;;
  esac

  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
  esac

  if [[ "$asset_platform" == "linux" ]] && version_at_least_0_3_1; then
    asset_platform="linux-${arch}-musl"
  else
    if [[ "$asset_platform" == "linux" ]] && using_musl; then
      fail "musl releases are not available before 0.3.1"
    fi
    asset_platform="${asset_platform}-${arch}"
  fi
}

validate_archive() {
  local archive=$1
  local member members types kind

  members="$(tar -tzf "$archive")" ||
    fail "invalid or corrupted archive"
  [[ -n "$members" ]] || fail "archive is empty"

  while IFS= read -r member; do
    case "$member" in
      chezmoi-tui | LICENSE | README.md) ;;
      *) fail "archive contains an unexpected member: $member" ;;
    esac
  done <<< "$members"

  types="$(LC_ALL=C tar -tvzf "$archive")" ||
    fail "invalid or corrupted archive"
  while IFS= read -r member; do
    kind=${member%"${member#?}"}
    [[ "$kind" == "-" ]] ||
      fail "archive contains a non-regular member"
  done <<< "$types"
}
