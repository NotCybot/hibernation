{
  description = "Hibernation";

  nixConfig = {
    extra-substituters = [
      "https://projects.cache.profidev.io"
    ];

    extra-trusted-public-keys = [
      "profidev.cachix.org:tg4xEn64UMdvA5jJYT8omo/CQHk8+spLyeGT2YAku70="
    ];

    connect-timeout = 5;
    fallback = true;
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    nix-filter.url = "github:numtide/nix-filter";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      nix-filter,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "hibernation";
          version = "0.4.3";

          src = nix-filter {
            root = ./.;
            include = [
              (nix-filter.lib.inDirectory "cli")
              (nix-filter.lib.inDirectory "shared")
              "backend/Cargo.toml"
              "backend/entity/Cargo.toml"
              "backend/migration/Cargo.toml"
              "Cargo.toml"
              "Cargo.lock"
            ];
          };

          RUSTFLAGS = "--cfg reqwest_unstable";
          buildAndTestSubdir = "cli";
          # reqwest refuses to build a client without CA certificates, the sandbox has none
          nativeCheckInputs = [ pkgs.cacert ];

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "harmonia-nar-3.0.0" = "sha256-BovRI3p2KXwQ6RF49NqLc0uKP/Jk+yA8E0eqScaIP68=";
            };
          };
        };
      }
    );
}
