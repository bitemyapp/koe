{ lib, rustPlatform, cmake, pkg-config, makeWrapper, pipewire, wl-clipboard, wtype, src }:
rustPlatform.buildRustPackage {
  pname = "aizome-koe";
  version = "0.1.0";
  inherit src;
  cargoLock.lockFile = ../Cargo.lock;
  nativeBuildInputs = [ cmake pkg-config makeWrapper ];
  # Cargo drives whisper.cpp's CMake build; this isn't a top-level CMake project.
  dontUseCmakeConfigure = true;
  WHISPER_DONT_GENERATE_BINDINGS = "1";
  GGML_NATIVE = "OFF";
  GGML_AVX = "OFF";
  GGML_AVX2 = "OFF";
  GGML_FMA = "OFF";
  GGML_F16C = "OFF";
  preCheck = ''
    patchShebangs tests
  '';
  postInstall = ''
    wrapProgram $out/bin/koe --prefix PATH : ${lib.makeBinPath [ pipewire wl-clipboard wtype ]}
    install -Dm644 data/aizome-koe.service $out/lib/systemd/user/aizome-koe.service
    substituteInPlace $out/lib/systemd/user/aizome-koe.service --replace-fail 'ExecStart=koe daemon' "ExecStart=$out/bin/koe daemon"
    install -Dm644 data/org.aizome.Koe.desktop $out/share/applications/org.aizome.Koe.desktop
    install -Dm644 data/org.aizome.Koe.svg $out/share/icons/hicolor/scalable/apps/org.aizome.Koe.svg
    install -Dm644 data/koe.example.toml $out/share/doc/koe/koe.example.toml
  '';
  meta = {
    description = "Local voice dictation for Aizome and Sekka";
    homepage = "https://github.com/bitemyapp/koe";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "koe";
  };
}
