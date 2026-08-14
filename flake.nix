# Dev shell for the messages backend (Rust). Enter with: nix develop
# Pure-Rust TLS (rustls) so there's no openssl/pkg-config native dep.
{
  description = "messages — Signal + Google Chat archive viewer backend";

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
            # The server binaries the ephemeral test DB runs. That harness is
            # `nix run ../dev-lint#with-test-db` — the gate's "tests (against a
            # real MariaDB)" row — and NOT a script in this repository: three
            # repos carried near-identical copies of one and they were folded
            # into dev-lint. This comment named the deleted copy until 2026-08-14,
            # which is how it came to be written a second time.
            pkgs.mariadb
            pkgs.nodejs_24 # Angular 22 frontend (frontend/)
            pkgs.pnpm # the frontend's installer; node ships npm too, ignore it
          ];
        };
      });
    };
}
