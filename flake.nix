{
  description = "A visual TUI wrapper around chezmoi";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      cargo = lib.importTOML ./Cargo.toml;
    in
    {
      packages = lib.genAttrs systems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = cargo.package.name;
            inherit (cargo.package) version;

            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [ pkgs.makeWrapper ];

            # The process-heavy integration suite runs in ci.yml on native
            # Linux and macOS runners; this derivation proves source packaging.
            doCheck = false;

            postInstall = ''
              wrapProgram "$out/bin/chezmoi-tui" \
                --prefix PATH : ${lib.makeBinPath [ pkgs.chezmoi ]}
            '';

            meta = {
              description = cargo.package.description;
              homepage = cargo.package.repository;
              license = lib.licenses.mit;
              mainProgram = "chezmoi-tui";
              platforms = lib.platforms.linux ++ lib.platforms.darwin;
            };
          };
        }
      );

      checks = lib.genAttrs systems (system: {
        default = self.packages.${system}.default;
      });

      apps = lib.genAttrs systems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/chezmoi-tui";
          meta.description = cargo.package.description;
        };
      });
    };
}
