{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          rustc
          cargo
          cargo-edit
          rustfmt
          clippy
          gcc
        ];
      };

      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "kiosk";
        version = cargoToml.workspace.package.version;
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [ "-p" "kiosk" ];
        doCheck = false;
      };
    };
}
