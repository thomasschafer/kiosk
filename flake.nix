{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in
    {
      devShells = forAllSystems (system: 
        let 
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              rustc
              cargo
              cargo-edit
              rustfmt
              clippy
              gcc
              tmux
            ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
            ];
          };
        }
      );

      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "kiosk";
            version = cargoToml.workspace.package.version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "kiosk" ];
            buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
            ];
            doCheck = false;
          };

          preview-fixture = pkgs.writeShellApplication {
            name = "preview-fixture";
            runtimeInputs = with pkgs; [
              coreutils
              gitMinimal
            ];
            text = builtins.readFile ./scripts/preview/fixture.sh;
          };

          preview-render = pkgs.writeShellApplication {
            name = "preview-render";
            runtimeInputs = with pkgs; [
              asciinema-agg
              coreutils
              ffmpeg-headless
              gnugrep
              jq
              tmux
            ];
            runtimeEnv.KIOSK_PREVIEW_FONT_DIR = "${pkgs.nerd-fonts.jetbrains-mono}/share/fonts/truetype/NerdFonts";
            text = builtins.readFile ./scripts/preview/render.sh;
          };
        }
      );
    };
}
