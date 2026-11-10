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

          shellHook = ''
            # mpv's Swift compiler needs the SDK from the same Xcode toolchain.
            export MACOS_SDK="$(/usr/bin/env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun --sdk macosx --show-sdk-path)"
            export MACOS_SDK_VERSION="$(/usr/bin/env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun --sdk macosx --show-sdk-version)"
            export JFN_MPV_LIB_DIR="$PWD/build/mpv-build"
            export JFN_CEF_TEST_FRAMEWORK="$PWD/build/Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework"
            export DYLD_FALLBACK_LIBRARY_PATH="$JFN_MPV_LIB_DIR''${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
          '';
        };
      });
  };
}
