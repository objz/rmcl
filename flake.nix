{
  description = "rmcl: A fully featured Minecraft TUI launcher";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    nixpkgs,
    flake-utils,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = nixpkgs.legacyPackages.${system};
      curseforgeApiKey = builtins.getEnv "CURSEFORGE_API_KEY";
      # minecraft's lwjgl/sdl natives dlopen the host's x11, opengl, vulkan,
      # input and audio stack, which isn't on the default loader path on nixos.
      # java is spawned as a child of rmcl, so a wrapper LD_LIBRARY_PATH
      # propagates all the way down. /run/opengl-driver/lib holds the mesa
      # drivers that libglvnd dispatches to.
      runtimeLibs =
        pkgs.lib.makeLibraryPath (with pkgs; [
          libX11
          libXext
          libXcursor
          libXrandr
          libXxf86vm
          libXi
          libXinerama
          libGL
          vulkan-loader
          libxkbcommon
          udev
          alsa-lib
          libpulseaudio
        ])
        + ":/run/opengl-driver/lib";
    in {
      packages.default = pkgs.rustPlatform.buildRustPackage {
        pname = "rmcl";
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
        src = pkgs.lib.cleanSource ./.;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [pkgs.jdk pkgs.makeWrapper];
        buildInputs = [pkgs.libxcb];

        env = pkgs.lib.optionalAttrs (curseforgeApiKey != "") {
          CURSEFORGE_API_KEY = curseforgeApiKey;
        };

        postFixup = ''
          wrapProgram $out/bin/rmcl --prefix LD_LIBRARY_PATH : ${runtimeLibs}
        '';

        meta = with pkgs.lib; {
          description = "A fully featured Minecraft TUI launcher";
          homepage = "https://github.com/objz/rmcl";
          license = licenses.gpl3Only;
          mainProgram = "rmcl";
        };
      };

      # `nix develop` + `cargo run --release` spawns java directly, bypassing
      # the wrapper above, so expose the same runtime libs in the dev shell.
      devShells.default = pkgs.mkShell {
        packages = with pkgs; [cargo rustc rustfmt clippy jdk pkg-config];
        buildInputs = [pkgs.libxcb];
        shellHook = ''
          export LD_LIBRARY_PATH="${runtimeLibs}:$LD_LIBRARY_PATH"
        '';
      };
    });
}
