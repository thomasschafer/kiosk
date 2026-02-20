{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      devShells.${system}.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          rustc
          cargo
          rustfmt
          clippy
          gcc
        ];
      };

      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "wts";
        version = "0.1.0";
        src = ./.;
        useFetchCargoVendor = true;
        cargoHash = "";
      };
    };
}
