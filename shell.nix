{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    cargo
    rustc
    pkg-config
    fontconfig
    wayland
    libxkbcommon
    libGL
    libx11
    libxcursor
    libxi
    libxrandr
    dbus
    patchelf
  ];

  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
    wayland
    libxkbcommon
    libGL
    libx11
    libxcursor
    libxi
    libxrandr
    fontconfig
    dbus.lib
  ]);

  shellHook = ''
    export RUSTFLAGS="-C link-arg=-Wl,-rpath,$LD_LIBRARY_PATH ''${RUSTFLAGS:-}"
  '';
}

