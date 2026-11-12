# Installation

chezmoi-tui supports Linux and macOS on x86_64 and aarch64. Native Windows is
not supported; Windows users can use the Linux instructions inside WSL.

`chezmoi` is a separate program and must be available through `PATH` or passed
with `--chezmoi`. Install it from the
[official chezmoi installation guide](https://www.chezmoi.io/install/) unless
you use the Homebrew formula, which declares a runtime dependency, or the Nix
flake, whose package wrapper includes `chezmoi`.

Version `0.3.1` adds distribution routes and fixes a process-output capture
race. Its configuration, CLI, and public Rust API remain compatible with `0.3.0`.

## Supported routes

<!-- markdownlint-disable MD013 -->
| Manager or artifact | Install source | Availability |
| --- | --- | --- |
| mise | `github:tetsuya-dev-jp/chezmoi-tui` backend | Available through the explicit GitHub backend; not a registry shorthand |
| asdf | This repository as a direct plugin URL | Direct plugin install; no separate plugin-registry registration is claimed |
| Aqua | `packaging/aqua*.yaml` | Repository-local custom registry and policy; not the standard Aqua registry |
| Homebrew | `Formula/chezmoi-tui.rb` in this repository | Repository-specific tap; not Homebrew core and no bottle is claimed |
| Nix | `flake.nix` in this repository | Flake source build; not nixpkgs |
| Cargo | crates.io package | Source build with `cargo install` |
| cargo-binstall | Release metadata in `Cargo.toml` | Versioned GitHub archives for supported target triples |
| GitHub archives | Six GNU/musl/macOS archives | Local binary install |
| Linux distribution packages | `.deb`, `.rpm`, `.pkg.tar.zst`, and `.apk` release assets | Local package installs only; no distribution repository is registered |
| Conda and Pixi | Local `.conda` release assets | Local package installs only; no conda-forge channel is published |
| Snap | Local `.snap` release assets | Local classic-confinement install; no Snap Store listing is published |
<!-- markdownlint-enable MD013 -->

Flatpak, MacPorts, and Guix packages are not published for `0.3.1`. There is no
AUR package. winget, Chocolatey, and Scoop packages are also not published, and
native Windows remains unsupported. Conda and Snap have local release assets,
but no conda-forge channel or Snap Store listing.

## mise

mise downloads the matching GitHub release archive through its explicit GitHub
backend:

```sh
mise use -g github:tetsuya-dev-jp/chezmoi-tui@0.3.1
```

To follow the newest release:

```sh
mise use -g github:tetsuya-dev-jp/chezmoi-tui@latest
```

To remove the global request and prune the installed version:

```sh
mise unuse -g github:tetsuya-dev-jp/chezmoi-tui@0.3.1
```

Use `mise exec github:tetsuya-dev-jp/chezmoi-tui@0.3.1 -- chezmoi-tui` for a
one-off invocation. Plain `mise use chezmoi-tui` is not a supported replacement:
the short-name registry does not contain this tool. `ubi:` is older and
deprecated; use the `github:` backend.

The GitHub route was exercised with mise `2026.8.5` using the `0.3.0` release
archive. The `0.3.1` release uses the same archive layout.

## Homebrew

This repository is both the tap and the source for its Formula:

```sh
brew tap tetsuya-dev-jp/chezmoi-tui https://github.com/tetsuya-dev-jp/chezmoi-tui
brew install tetsuya-dev-jp/chezmoi-tui/chezmoi-tui
```

The formula builds from the `v0.3.1` Git tag, declares Rust as a build
dependency, and declares `chezmoi` as a runtime dependency. Update or remove it
with:

```sh
brew update
brew upgrade tetsuya-dev-jp/chezmoi-tui/chezmoi-tui
brew uninstall tetsuya-dev-jp/chezmoi-tui/chezmoi-tui
```

Optionally remove the tap after uninstalling:

```sh
brew untap tetsuya-dev-jp/chezmoi-tui
```

This is a custom tap, not a Homebrew core formula. Do not expect
`brew install chezmoi-tui` without the fully qualified tap name.

## Nix

The repository flake provides the default package and app for
`x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, and `aarch64-darwin`. It
builds the TUI from source and includes a `chezmoi` wrapper.

Run without installing:

```sh
nix run github:tetsuya-dev-jp/chezmoi-tui/v0.3.1
```

Install into the current Nix profile:

```sh
nix profile install github:tetsuya-dev-jp/chezmoi-tui/v0.3.1
```

Inspect the profile entry before upgrading or removing it:

```sh
nix profile list
```

Use the listed entry with `nix profile upgrade <entry>` or
`nix profile remove <entry>`. The versioned installable is pinned to `v0.3.1`;
to change releases, install the new tag as needed.

For a NixOS or Home Manager configuration, add this flake as an input and use
its default package for the host system:

```nix
inputs.chezmoi-tui.url = "github:tetsuya-dev-jp/chezmoi-tui/v0.3.1";
```

This flake does not make chezmoi-tui part of nixpkgs. It does not support
`nix run nixpkgs#chezmoi-tui`.

The Linux x86_64 flake was built and its wrapped `chezmoi-tui --version` was
smoke-tested with Nix `2.35.6` and Rust `1.95`. This does not claim runtime
testing on the other listed systems.

## asdf

Add this repository as an asdf plugin, then install and select `0.3.1`:

```sh
asdf plugin add chezmoi-tui https://github.com/tetsuya-dev-jp/chezmoi-tui
asdf install chezmoi-tui 0.3.1
asdf set -u chezmoi-tui 0.3.1
```

`asdf set -u` is the user-level selection in modern asdf `0.16` and newer. The
obsolete `asdf global` command is unnecessary.

To update the plugin and remove the installed version:

```sh
asdf plugin update chezmoi-tui
asdf set -u chezmoi-tui system
asdf uninstall chezmoi-tui 0.3.1
asdf plugin remove chezmoi-tui
```

Current asdf has no `asdf unset` command. The `system` selection stops using
the managed version for the user-level configuration. If you no longer want
the entry in `~/.tool-versions`, edit that file directly before uninstalling.

The plugin is added by URL. This repository does not claim separate registration
in an asdf plugin registry.

## Aqua

This repository provides a local Aqua registry instead of an entry in Aqua's
standard registry. Aqua's default policy permits only its standard registry,
and it does not discover the repository policy automatically. Point every
Aqua command for this configuration at the policy explicitly:

```sh
AQUA_POLICY_CONFIG="$PWD/packaging/aqua-policy.yaml" \
  aqua --config packaging/aqua.yaml install
```

`packaging/aqua.yaml` loads the custom registry named `chezmoi-tui` from
`packaging/aqua-registry.yaml`; `packaging/aqua-policy.yaml` explicitly allows
that repository-owned registry. The registry entry is pinned to the
`tetsuya-dev-jp/chezmoi-tui` repository at `v0.3.1`. Keep these files together
when using this configuration elsewhere.

On Linux, Aqua requires the static musl artifact. This makes the Linux route
usable on Alpine as well as systems with a compatible glibc runtime. That final
`v0.3.1` asset cannot be exercised through the published Aqua configuration
until the release exists. The earlier development install and execution check
used the `v0.3.0` GNU archive and is not evidence that the final `v0.3.1`
static Linux artifact has already passed.

To uninstall the linked package and downloaded payload, use the registry-qualified
name and remove both links and packages:

```sh
AQUA_POLICY_CONFIG="$PWD/packaging/aqua-policy.yaml" \
  aqua --config packaging/aqua.yaml remove --mode pl \
  chezmoi-tui,tetsuya-dev-jp/chezmoi-tui
```

When changing releases, update the package version and registry reference in
`aqua.yaml` and the matching reference expression in `aqua-policy.yaml`, then
rerun the install command above. Do not expect a standard-registry command
such as `aqua g chezmoi-tui` to work.

## cargo-binstall and Cargo

Install the prebuilt archive selected for the host target:

```sh
cargo binstall chezmoi-tui --version 0.3.1
```

The release metadata maps the GNU and musl archives for Linux x86_64 and
aarch64, plus the two macOS targets. Update with the same command for a newer
version. Uninstall it with:

```sh
cargo uninstall chezmoi-tui
```

To build from the locked crates.io source:

```sh
cargo install chezmoi-tui --version 0.3.1 --locked
```

Source builds require Rust `1.88` or newer. Update with the same command for a
newer version and add `--force` if needed. Uninstall with
`cargo uninstall chezmoi-tui`.

## Release archives

The `v0.3.1` release asset set includes:

- `chezmoi-tui_0.3.1_linux-x86_64.tar.gz`
- `chezmoi-tui_0.3.1_linux-aarch64.tar.gz`
- `chezmoi-tui_0.3.1_linux-x86_64-musl.tar.gz`
- `chezmoi-tui_0.3.1_linux-aarch64-musl.tar.gz`
- `chezmoi-tui_0.3.1_macos-x86_64.tar.gz`
- `chezmoi-tui_0.3.1_macos-aarch64.tar.gz`

Choose the GNU archive for a normal glibc Linux system and the musl archive for
musl systems such as Alpine. Then install the binary into a directory already
on `PATH`:

```sh
tar -xzf chezmoi-tui_0.3.1_linux-x86_64.tar.gz
mkdir -p "$HOME/.local/bin"
mv chezmoi-tui "$HOME/.local/bin/chezmoi-tui"
```

Replace the archive and extracted-binary choices for another architecture.
Remove a local binary install with:

```sh
rm "$HOME/.local/bin/chezmoi-tui"
```

## Linux package assets

The release asset set defines matching packages for x86_64 and aarch64:

- `chezmoi-tui_0.3.1_amd64.deb`
- `chezmoi-tui_0.3.1_arm64.deb`
- `chezmoi-tui_0.3.1_x86_64.rpm`
- `chezmoi-tui_0.3.1_aarch64.rpm`
- `chezmoi-tui_0.3.1_x86_64.pkg.tar.zst`
- `chezmoi-tui_0.3.1_aarch64.pkg.tar.zst`
- `chezmoi-tui_0.3.1_x86_64.apk`
- `chezmoi-tui_0.3.1_aarch64.apk`

These packages contain the static musl binary. They are GitHub release assets:
they are not registered in the apt, dnf/yum, zypper, pacman, or apk repositories
for any distribution. Install `chezmoi` separately first.

Release CI is configured to install the generated Debian, RPM, and APK
packages on native Linux x86_64 and aarch64 runners. Arch package installation
is checked on x86_64 only; aarch64 package metadata and paths are still checked.

Debian or Ubuntu:

```sh
sudo apt install ./chezmoi-tui_0.3.1_amd64.deb
sudo apt remove chezmoi-tui
```

Fedora, RHEL-family systems, or openSUSE:

```sh
sudo dnf install ./chezmoi-tui_0.3.1_x86_64.rpm
sudo dnf remove chezmoi-tui

sudo yum install ./chezmoi-tui_0.3.1_x86_64.rpm
sudo yum remove chezmoi-tui

sudo zypper install ./chezmoi-tui_0.3.1_x86_64.rpm
sudo zypper remove chezmoi-tui
```

Arch Linux:

```sh
sudo pacman -U ./chezmoi-tui_0.3.1_x86_64.pkg.tar.zst
sudo pacman -Rns chezmoi-tui
```

Alpine Linux:

```sh
sudo apk add --allow-untrusted ./chezmoi-tui_0.3.1_x86_64.apk
sudo apk del chezmoi-tui
```

APK packages uploaded directly to the GitHub release are not supplied through
an Alpine repository. `--allow-untrusted` is therefore required for these local
assets.

On aarch64 systems, substitute the `arm64`, `aarch64`, or `aarch64.apk` asset
shown above. Update a local package by downloading the newer matching asset and
running the same install command.

## Conda and Pixi

The release creates these local Conda packages from the static musl binaries:

- `chezmoi-tui-0.3.1-linux_64_0.conda` for x86_64.
- `chezmoi-tui-0.3.1-linux_aarch64_0.conda` for aarch64.

These files are release assets. No package is published to conda-forge or
another Conda channel. Install the matching file into an isolated Conda
environment:

```sh
conda create -n chezmoi-tui ./chezmoi-tui-0.3.1-linux_64_0.conda
conda activate chezmoi-tui
```

`--offline` may be added to `conda create` to prevent configured channels from
being contacted. The package itself needs no channel. To use the same local
asset in a Pixi project configured for the matching platform, pass an absolute
path:

```sh
pixi init
pixi add /absolute/path/to/chezmoi-tui-0.3.1-linux_64_0.conda
pixi run chezmoi-tui --version
```

The x86_64 package was installed and run successfully with micromamba `2.10.0`
using `--offline` and with Pixi `0.84.2` in a workspace with no channels. Release
CI is configured to create both architectures from native musl artifacts.
`chezmoi` remains a separate prerequisite.

To update, remove the old local package and add the newer matching asset. To
remove the Conda environment and everything in it:

```sh
conda deactivate
conda env remove -n chezmoi-tui
```

For Pixi, remove the dependency from the current project:

```sh
pixi remove chezmoi-tui
```

## Snap

The release also creates local Snap packages for both Linux architectures:

- `chezmoi-tui_0.3.1_amd64.snap` for x86_64.
- `chezmoi-tui_0.3.1_arm64.snap` for aarch64.

The Snap uses classic confinement so it can invoke the separately installed
`chezmoi` command and use the user's chezmoi configuration and source tree. It
is not listed in the Snap Store and is unsigned for store distribution, so the
local file must be installed with both `--dangerous` and `--classic`:

```sh
sudo snap install --dangerous --classic ./chezmoi-tui_0.3.1_amd64.snap
/snap/bin/chezmoi-tui --version
```

The x86_64 Snap metadata identifies `core24` and classic confinement, and its
installed paths were inspected on the build host. A real Snap installation
requires a Snapd-enabled Linux system. Release CI is configured to install and
run the Snap on native Ubuntu 24.04 x86_64 and aarch64 runners. Use the `arm64`
file on aarch64.

Update a local Snap by removing the old installation and installing the newer
matching file with the same command. Uninstall it with:

```sh
sudo snap remove chezmoi-tui
```

## Availability boundaries

The following names and registries must not be used as installation claims for
`0.3.1`:

- Homebrew core (`brew install chezmoi-tui`)
- nixpkgs (`nix run nixpkgs#chezmoi-tui`)
- the standard Aqua registry
- a plain mise registry shorthand
- an asdf central plugin-registry package
- apt, dnf/yum, zypper, pacman, or apk package repositories
- AUR
- conda-forge or another central Conda channel
- the Snap Store
- Flatpak, MacPorts, or Guix
- winget, Chocolatey, or Scoop

The supported routes above use the project's releases, repository configuration,
custom tap/flake, or explicit manager URLs. The Conda and Snap packages are
downloaded from this repository's GitHub release and installed from the local
file.
