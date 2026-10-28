//! On Windows, built with MSYS2's GNU toolchain (as the installer is), puts
//! Fisga's icon into `fisga-gui.exe` with `windres`: the taskbar, Explorer
//! and the Start menu show it.

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../packaging/windows");
    println!("cargo:rerun-if-changed={}", dir.join("fisga.rc").display());
    println!("cargo:rerun-if-changed={}", dir.join("fisga.ico").display());
    let target = |key: &str| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") != "windows" || target("CARGO_CFG_TARGET_ENV") != "gnu" {
        return;
    }
    let object = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("fisga-icon.o");
    let built = std::process::Command::new("windres")
        .arg("--include-dir")
        .arg(&dir)
        .arg("--input")
        .arg(dir.join("fisga.rc"))
        .args(["--output-format=coff", "--output"])
        .arg(&object)
        .status()
        .is_ok_and(|status| status.success());
    match built {
        // An object named on the command line is linked in whole: its resources too.
        true => println!("cargo:rustc-link-arg-bins={}", object.display()),
        false => println!("cargo:warning=windres falhou: fisga-gui.exe fica sem o ícone"),
    }
}
