use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=installer.manifest");
    println!("cargo:rerun-if-changed=installer.rc");
    println!("cargo:rerun-if-changed=../icons/icon.ico");
    println!("cargo:rerun-if-changed=../c_proxy/amp_proxy.dll");
    println!("cargo:rerun-if-env-changed=AM_LIGHT_BIN_PATH");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    // Resolve built AM-Light.exe payload without requiring the binary to be tracked in git
    let payload_src = env::var_os("AM_LIGHT_BIN_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            let parent_exe = manifest_dir.join("..").join("AM-Light.exe");
            parent_exe.is_file().then_some(parent_exe)
        });

    let payload_dst = out_dir.join("payload_am_light.exe");
    if let Some(src) = payload_src {
        println!("cargo:rerun-if-changed={}", src.display());
        fs::copy(&src, &payload_dst).expect("copy AM-Light.exe payload");
    } else {
        fs::write(&payload_dst, b"PLACEHOLDER").expect("write placeholder payload");
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let res_obj = out_dir.join("am_light_setup_res.o");
        let rc_path = manifest_dir.join("installer.rc");
        let status = Command::new("windres")
            .current_dir(&manifest_dir)
            .arg("--input")
            .arg(&rc_path)
            .arg("--output")
            .arg(&res_obj)
            .arg("--output-format=coff")
            .status()
            .expect("failed to invoke windres.exe");
        assert!(status.success(), "windres failed on installer.rc");
        println!("cargo:rustc-link-arg={}", res_obj.display());
    }
}
