# Dev shell for the viewer and the archiver. Enter with: nix develop
# rustls, so no openssl or pkg-config.
{
  description = "messages — Signal, Google Chat, IRC and Telegram archive";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in {
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rust-analyzer
            pkgs.rustfmt
            pkgs.clippy
            pkgs.sqlx-cli
            # MariaDB for `nix run ../dev-lint#with-test-db`, the gate's test row.
            pkgs.mariadb
            pkgs.nodejs_24 # Angular 22 frontend (frontend/)
            pkgs.pnpm # the frontend's installer; node ships npm too, ignore it
          ];
        };
      });
    };
}
