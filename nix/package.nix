{
  lib,
  rust,
  makeRustPlatform,
}:
let
  rustPlatform = makeRustPlatform {
    cargo = rust;
    rustc = rust;
  };
in
rustPlatform.buildRustPackage {
  pname = "r-boot";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        base = baseNameOf path;
      in
      base != "target" && base != ".cache" && base != ".direnv";
  };

  cargoLock.lockFile = ../Cargo.lock;

  # cargo-auditable's version-info injection produces an object file the
  # UEFI COFF linker doesn't understand.
  auditable = false;

  # rustPlatform's own cargoBuildHook always targets the derivation's
  # stdenv.hostPlatform (nixpkgs' cross machinery), which has no bearing on
  # rust-overlay's `x86_64-unknown-uefi` target support used here. Drive
  # cargo directly instead; cargoSetupHook has already vendored the
  # dependencies and configured cargo for an offline, `--target`-independent
  # build.
  buildPhase = ''
    runHook preBuild
    cargo build --release --target x86_64-unknown-uefi --offline -j "$NIX_BUILD_CORES"
    runHook postBuild
  '';

  # A no_std UEFI binary cannot run the host test harness.
  doCheck = false;

  installPhase = ''
    runHook preInstall
    install -D target/x86_64-unknown-uefi/release/r-boot.efi $out/EFI/BOOT/BOOTX64.EFI
    runHook postInstall
  '';

  meta = {
    description = "Rust UEFI Limine-protocol and Linux EFI handover bootloader";
    homepage = "https://github.com/benedikt-weyer/r-boot";
    license = with lib.licenses; [
      mit
      asl20
    ];
    platforms = [ "x86_64-linux" ];
    mainProgram = "BOOTX64.EFI";
  };
}
