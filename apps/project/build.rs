//! Windows icon of `project.exe` (`assets/icon.ico`), compiled with mingw's `windres` when the
//! target is `*-windows-gnu`. Other targets: nothing to do.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" {
        return;
    }
    if env != "gnu" {
        println!("cargo:warning=no icon embedded: only the windows-gnu target runs windres");
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let icon = manifest.join("../../assets/icon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));
    let rc = out.join("icon.rc");
    let icon_path = icon.display().to_string().replace('\\', "/");
    std::fs::write(&rc, format!("1 ICON \"{icon_path}\"\n")).expect("write icon.rc");
    let res = out.join("icon.res.o");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let windres =
        std::env::var("WINDRES").unwrap_or_else(|_| format!("{arch}-w64-mingw32-windres"));
    let status = Command::new(&windres)
        .arg("-O")
        .arg("coff")
        .arg(&rc)
        .arg("-o")
        .arg(&res)
        .status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", res.display()),
        Ok(s) => panic!("{windres} failed: {s}"),
        Err(e) => println!("cargo:warning=no icon embedded: cannot run {windres}: {e}"),
    }
}
