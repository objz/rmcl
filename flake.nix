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
      # release binaries have the curseforge key compiled in; source builds
      # don't, so forward the caller's key when building impurely.
      curseforgeApiKey = builtins.getEnv "CURSEFORGE_API_KEY";
      # minecraft's bundled lwjgl natives dlopen the host's x11 stack, which
      # isn't on the default loader path on nixos. java is spawned as a child
      # of rmcl, so a wrapper LD_LIBRARY_PATH propagates all the way down.
      runtimeLibs = pkgs.lib.makeLibraryPath (with pkgs; [
        libX11
        libXext
        libXcursor
        libXrandr
        libXxf86vm
      ]);
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
    });
}
