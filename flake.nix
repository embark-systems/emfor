{
  description = "emfor: a health-check daemon that publishes backend state to a local store";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      # The tests read docs, RELEASING.md and the FreeBSD port, so keep the whole tree
      # except build output and VCS state.
      src = nixpkgs.lib.cleanSourceWith {
        src = ./.;
        filter =
          path: type:
          let
            base = baseNameOf path;
          in
          !(builtins.elem base [
            "target"
            ".git"
            "result"
          ]);
      };
      meta = pkgs: {
        homepage = "https://github.com/embark-systems/emfor";
        license = pkgs.lib.licenses.bsd2;
        platforms = pkgs.lib.platforms.unix;
      };
    in
    {
      packages = forAll (pkgs: rec {
        # The daemon and the emfor-check helper.
        emfor = pkgs.rustPlatform.buildRustPackage {
          pname = "emfor";
          inherit version src;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "--package"
            "emfor-daemon"
            "--package"
            "emfor-check"
          ];
          cargoTestFlags = [
            "--package"
            "emfor-daemon"
            "--package"
            "emfor-check"
          ];
          # aws-lc-sys builds C code with cmake.
          nativeBuildInputs = [
            pkgs.cmake
            pkgs.pkg-config
          ]
          # The plugin tests look for their child processes with pgrep.
          ++ pkgs.lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.procps;
          dontUseCmakeConfigure = true;
          # nixpkgs' cargo setup hook exports AWS_LC_SYS_USE_SYSTEM=1, which makes aws-lc-sys look for a
          # system AWS-LC. Build the vendored copy that Cargo.lock pins.
          preBuild = ''
            unset AWS_LC_SYS_USE_SYSTEM
          '';
          # buildRustPackage also exports an empty RUST_LOG. emfor treats any RUST_LOG as
          # "use this filter", so the empty value silences the daemon's error messages and
          # the tests that read them fail.
          preCheck = ''
            unset AWS_LC_SYS_USE_SYSTEM RUST_LOG
          '';
          __darwinAllowLocalNetworking = true;
          meta = (meta pkgs) // {
            description = "Health-check daemon that publishes backend state to a local store";
            mainProgram = "emfor";
          };
        };

        # The C client library. It bundles its own LMDB on purpose: it has to match the
        # daemon's lock format, so it must never link a system liblmdb.
        libemfor = pkgs.rustPlatform.buildRustPackage {
          pname = "libemfor";
          inherit version src;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [
            pkgs.cargo-c
            pkgs.pkg-config
          ];
          buildPhase = ''
            runHook preBuild
            cd clients/rust
            cargo cbuild --release --frozen --features capi \
              --prefix=$out --libdir=$out/lib --pkgconfigdir=$out/lib/pkgconfig
            runHook postBuild
          '';
          doCheck = false;
          installPhase = ''
            runHook preInstall
            cargo cinstall --release --frozen --features capi \
              --prefix=$out --libdir=$out/lib --pkgconfigdir=$out/lib/pkgconfig
            runHook postInstall
          '';
          meta = (meta pkgs) // {
            description = "C client library for the emfor state store";
          };
        };

        default = emfor;
      });

      nixosModules.default = import ./nix/module.nix { inherit self; };
    };
}
