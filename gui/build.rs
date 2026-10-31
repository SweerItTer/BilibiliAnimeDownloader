use std::path::{Path, PathBuf};
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }
    let kits = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from)?;
    let bin = kits.join("Windows Kits/10/bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&bin)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join("x64/rc.exe").is_file())
        .collect();
    versions.sort();
    versions.pop().map(|path| path.join("x64/rc.exe"))
}

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/icon.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found; the executable keeps the default icon");
        return;
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("icon.res");
    let source = Path::new("assets/icon.rc");
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg(format!("/fo{}", out.display()))
        .arg(source)
        .status();
    match status {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg-bins={}", out.display());
        }
        _ => println!("cargo:warning=failed to compile asset resource; using the default icon"),
    }
}
