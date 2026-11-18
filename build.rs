use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_profile_dir(out_dir: &Path) -> Option<PathBuf> {
    let mut prev = out_dir;
    for anc in out_dir.ancestors().skip(1) {
        if prev.file_name().and_then(|n| n.to_str()) == Some("build") {
            return Some(anc.to_path_buf());
        }
        prev = anc;
    }
    None
}

fn stage_amp_proxy_dll(manifest_dir: &Path) {
    let src_dll = manifest_dir.join("c_proxy").join("amp_proxy.dll");
    if !src_dll.exists() {
        return;
    }

    if let Ok(out_dir_str) = env::var("OUT_DIR") {
        let out_dir = PathBuf::from(out_dir_str);
        let _ = fs::copy(&src_dll, out_dir.join("amp_proxy.dll"));
        if let Some(profile_dir) = find_profile_dir(&out_dir) {
            let _ = fs::copy(&src_dll, profile_dir.join("amp_proxy.dll"));
            let deps_dir = profile_dir.join("deps");
            if deps_dir.exists() {
                let _ = fs::copy(&src_dll, deps_dir.join("amp_proxy.dll"));
            }
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=c_proxy/amp_proxy.dll");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    stage_amp_proxy_dll(&manifest_dir);

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    if target_env == "gnu" {
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
        let res_obj = out_dir.join("am_light_res.o");
        let rc_path = manifest_dir.join("app.rc");
        let status = Command::new("windres")
            .current_dir(&manifest_dir)
            .arg("--input")
            .arg(&rc_path)
            .arg("--output")
            .arg(&res_obj)
            .arg("--output-format=coff")
            .status()
            .expect("failed to invoke windres.exe");
        assert!(status.success(), "windres failed to compile app.rc");
        // Pass COFF resource object directly to linker so RT_MANIFEST #1 (with amp_proxy.dll SxS COM stubs)
        // overrides rustc's default MinGW manifest in both bin and test executables!
        println!("cargo:rustc-link-arg={}", res_obj.display());
    } else if target_env == "msvc" {
        let manifest_path = manifest_dir.join("app.manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest_path.display());
        println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
        let _ = embed_resource::compile("app.rc", embed_resource::NONE).manifest_optional();
    }
}
