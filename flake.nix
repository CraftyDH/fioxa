{
  description = "rust compiler";

  inputs = {
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
      in
      {
        devShells.default = pkgs.mkShell {
          name = "rust";
          env.OVMF = "${pkgs.OVMF.fd}/FV";
          packages = [
            pkgs.git
            pkgs.qemu_kvm
          ];
          buildInputs = [
            (pkgs.rust-bin.nightly."2025-10-03".default.override {
              extensions = [
                "rustc"
                "cargo"
                "clippy"
                "rustfmt"
                "rust-analyzer"
                "rust-src"
              ];
            })
          ];
        };
      }
    );
}
