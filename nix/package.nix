{ lib, rustPlatform, cmake, pkg-config, makeWrapper, openblas, fontconfig, wayland, vulkan-headers, vulkan-loader, shaderc, pipewire, wl-clipboard, wtype, src, vulkanSupport ? false }:
rustPlatform.buildRustPackage {
  pname = "aizome-koe";
  version = "0.1.0";
  inherit src;
  cargoLock.lockFile = ../Cargo.lock;
  cargoLock.outputHashes = {
    "aizome-kit-0.1.0" = "sha256-cwpWQM14Dyby+njXfqkg2x+KZhTIwK4yxQ3y39AD7kQ=";
    "sidestep-ui-0.0.1" = "sha256-HGpKhuaSNMl8tmy39RKd/ZQbw2X27OaIcSp7S/nl8I4=";
  };
  nativeBuildInputs = [ cmake pkg-config makeWrapper ] ++ lib.optionals vulkanSupport [ shaderc rustPlatform.bindgenHook ];
  # OpenBLAS selects optimized kernels for the current CPU at runtime;
  # the rest of whisper.cpp stays portable to baseline CPUs.
  buildInputs = [ openblas fontconfig wayland ] ++ lib.optionals vulkanSupport [ vulkan-headers vulkan-loader ];
  buildFeatures = [ "openblas" ] ++ lib.optional vulkanSupport "vulkan";
  BLAS_INCLUDE_DIRS = "${lib.getDev openblas}/include";
  OPENBLAS_PATH = "${lib.getLib openblas}";
  # Cargo drives whisper.cpp's CMake build; this isn't a top-level CMake project.
  dontUseCmakeConfigure = true;
  # The supplied bindings omit Vulkan device functions; regenerate for GPU builds.
  WHISPER_DONT_GENERATE_BINDINGS = "1";
  # ggml's shader generator starts a thread per shader variant (over a thousand),
  # each with a stack the size of the soft stack limit. Under nix-daemon's 64 MiB
  # limit, its glslc forks fail on a loaded machine and the missing shaders only
  # show up as undefined symbols at link time; the default 8 MiB is plenty.
  preBuild = lib.optionalString vulkanSupport ''
    unset WHISPER_DONT_GENERATE_BINDINGS
    ulimit -Ss 8192
  '';
  GGML_NATIVE = "OFF";
  GGML_AVX = "OFF";
  GGML_AVX2 = "OFF";
  GGML_FMA = "OFF";
  GGML_F16C = "OFF";
  preCheck = ''
    patchShebangs tests
  '';
  postInstall = ''
    wrapProgram $out/bin/koe --prefix PATH : ${lib.makeBinPath [ pipewire wl-clipboard wtype ]} --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath [ fontconfig wayland ]}
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
