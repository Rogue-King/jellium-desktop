{
  description = "Jellium Desktop macOS development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { nixpkgs, ... }: {
    devShells = nixpkgs.lib.genAttrs [ "aarch64-darwin" "x86_64-darwin" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        libraries = with pkgs; [
          ffmpeg
          libplacebo
          libass
          luajit
          vulkan-loader
          vulkan-headers
          moltenvk
          lcms2
          libunibreak
          zimg
        ];
        dependencyPrefix = pkgs.symlinkJoin {
          name = "jellium-macos-deps";
          paths = map pkgs.lib.getLib libraries;
        };
      in {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            just
            cmake
            meson
            ninja
            pkg-config
            python3
            create-dmg
          ];

          buildInputs = libraries;

          JFN_MACOS_DEPS_PREFIX = dependencyPrefix;
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        };
      });
  };
}
