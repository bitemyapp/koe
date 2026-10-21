{
  description = "Koe: local Rust voice dictation for Aizome and Sekka";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/7fd4004d0511991fff3a0524856a9be09e28c5f0";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      packages = forAll (pkgs: rec {
        koe = pkgs.callPackage ./nix/package.nix { src = self; };
        default = koe;
      });
      apps = forAll (pkgs: {
        default = { type = "app"; program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.koe}/bin/koe"; meta.description = "Koe voice dictation"; };
      });
      nixosModules.default = import ./nix/module.nix { inherit self; };
      checks = forAll (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.koe;
        module = let
          evaluated = nixpkgs.lib.nixosSystem {
            system = pkgs.stdenv.hostPlatform.system;
            modules = [ self.nixosModules.default {
              programs.aizome.koe.enable = true;
              programs.aizome.koe.settings.engine.language = "ja";
              system.stateVersion = "26.05";
              boot.loader.grub.enable = false;
              fileSystems."/" = { device = "none"; fsType = "tmpfs"; };
            } ];
          };
        in pkgs.writeText "koe-module-test" evaluated.config.systemd.user.services.aizome-koe.serviceConfig.ExecStart;
      });
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy cmake pkg-config pipewire wl-clipboard wtype ];
          WHISPER_DONT_GENERATE_BINDINGS = "1";
        };
      });
    };
}
