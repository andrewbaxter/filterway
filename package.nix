{ pkgs, lib }:
let
  fenix = import (fetchTarball "https://github.com/nix-community/fenix/archive/1a79901b0e37ca189944e24d9601c8426675de50.zip") { };
  naersk = pkgs.callPackage (fetchTarball "https://github.com/nix-community/naersk/archive/378614f37a6bee5a3f2ef4f825a73d948d3ae921.zip") (
    let
      toolchain = fenix.combine [
        fenix.latest.rustc
        fenix.latest.cargo
      ];
    in
    {
      rustc = toolchain;
      cargo = toolchain;
    }
  );
in
naersk.buildPackage {
  root = ./.;
}
