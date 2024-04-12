{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = [
    pkgs.bashInteractive
    pkgs.wabt
    pkgs.rustup
    pkgs.cargo-lambda
  ];
}
