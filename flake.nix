{
  description = "Zinkite, a programming language, and its compiler, zink";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;

      # Linux only: the runtime reserves its data stacks with mmap's MAP_NORESERVE.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEach = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # The compiler's files, and whatever else from the repository is named.
      source =
        more:
        lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions (
            [
              ./compiler/Cargo.toml
              ./compiler/Cargo.lock
              ./compiler/src
              ./compiler/lib
              ./compiler/runtime
              ./compiler/tests
            ]
            ++ more
          );
        };

      package =
        {
          lib,
          rustPlatform,
          makeWrapper,
          clang,
        }:
        rustPlatform.buildRustPackage {
          pname = "zink";
          version = (lib.importTOML ./compiler/Cargo.toml).package.version;

          # The standard library and the runtime header are compiled into the binary.
          src = source [ ];
          sourceRoot = "source/compiler";
          cargoLock.lockFile = ./compiler/Cargo.lock;

          # zink builds every program with a C compiler, and reads C headers with clang.
          # This clang goes at the end of the path, so $CC or a clang of your own comes
          # first.
          nativeBuildInputs = [ makeWrapper ];
          postInstall = ''
            wrapProgram $out/bin/zink --suffix PATH : ${lib.makeBinPath [ clang ]}
          '';

          # The tests run in `nix flake check` instead (checks.tests below), so a system
          # rebuild doesn't wait on them.
          doCheck = false;

          meta = {
            description = "The Zinkite compiler";
            homepage = "https://github.com/TheLinuxSeal/Zinkite";
            license = lib.licenses.asl20;
            mainProgram = "zink";
            platforms = systems;
          };
        };
    in
    {
      packages = forEach (pkgs: rec {
        zink = pkgs.callPackage package { };
        default = zink;
      });

      overlays.default = final: prev: { zink = final.callPackage package { }; };

      checks = forEach (pkgs: {
        tests = self.packages.${pkgs.stdenv.hostPlatform.system}.zink.overrideAttrs (old: {
          doCheck = true;
          # Some tests read the examples and GRAMMAR.md's complete example.
          src = source [
            ./examples
            ./GRAMMAR.md
          ];
          nativeCheckInputs = (old.nativeCheckInputs or [ ]) ++ [ pkgs.clang ];
          # The tests build programs, which zink caches under $XDG_CACHE_HOME.
          preCheck = ''
            export XDG_CACHE_HOME=$(mktemp -d)
          '';
        });
      });
    };
}
