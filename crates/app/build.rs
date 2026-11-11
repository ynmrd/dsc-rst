fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    windows_resources();
    // Cross-compiling from Linux: cfg(windows) is false for build scripts, so check the target.
    #[cfg(not(windows))]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cross_windows_resources();
    }
}

#[cfg(windows)]
fn windows_resources() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon.ico");
    res.set("ProductName", "Discord Native");
    res.set("FileDescription", "Discord Native");
    res.set("CompanyName", "Discord Native contributors");
    res.set("LegalCopyright", "MIT License. Not affiliated with Discord Inc.");
    if let Err(e) = res.compile() {
        println!("cargo:warning=embedding Windows resources failed: {e}");
    }
}

/// Uses mingw's windres to embed the icon and version info when cross-compiling.
#[cfg(not(windows))]
fn cross_windows_resources() {
    use std::path::PathBuf;
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let icon = manifest.join("../../assets/icon.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let parts: Vec<u16> = version.split('.').filter_map(|p| p.parse().ok()).collect();
    let v = |i: usize| parts.get(i).copied().unwrap_or(0);
    let rc = format!(
        "1 ICON \"{icon}\"\n\
         1 VERSIONINFO\nFILEVERSION {a},{b},{c},0\nPRODUCTVERSION {a},{b},{c},0\nBEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904b0\"\n  BEGIN\n\
           VALUE \"ProductName\", \"Discord Native\"\n   VALUE \"FileDescription\", \"Discord Native\"\n   VALUE \"FileVersion\", \"{version}\"\n\
           VALUE \"ProductVersion\", \"{version}\"\n   VALUE \"LegalCopyright\", \"MIT License. Not affiliated with Discord Inc.\"\n  END\n END\n\
         BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\nEND\n",
        icon = icon.display().to_string().replace('\\', "/"),
        a = v(0),
        b = v(1),
        c = v(2),
    );
    let rc_path = out.join("resources.rc");
    let res_path = out.join("resources.o");
    std::fs::write(&rc_path, rc).expect("write rc");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
    match std::process::Command::new(&windres).arg(&rc_path).arg("-O").arg("coff").arg("-o").arg(&res_path).status() {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        }
        other => println!("cargo:warning=windres unavailable ({other:?}); the exe will have no icon"),
    }
}
