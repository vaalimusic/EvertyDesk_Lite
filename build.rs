use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=EVERTYDESK_NV_CODEC_SDK");
    println!("cargo:rerun-if-env-changed=NV_CODEC_SDK");
    println!("cargo:rerun-if-env-changed=NVIDIA_VIDEO_CODEC_SDK");
    println!("cargo:rustc-check-cfg=cfg(nv_codec_sdk_present)");
    println!("cargo:rustc-check-cfg=cfg(nvenc_api_ffi)");

    let Some(sdk) = find_nv_codec_sdk() else {
        return;
    };

    println!("cargo:rerun-if-changed={}", sdk.display());
    println!("cargo:rustc-cfg=nv_codec_sdk_present");
    println!(
        "cargo:rustc-env=EVERTYDESK_NV_CODEC_SDK_PATH={}",
        sdk.display()
    );
    if let Some(version) = sdk_version(&sdk) {
        println!("cargo:rustc-env=EVERTYDESK_NV_CODEC_SDK_VERSION={version}");
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if env::var_os("CARGO_FEATURE_LIVE_NVENC_SDK").is_some()
        && matches!(target_os.as_str(), "linux" | "windows")
    {
        println!("cargo:rustc-cfg=nvenc_api_ffi");
        if target_os == "windows" {
            compile_nvenc_windows_shim(&sdk);
        }
    }
}

fn find_nv_codec_sdk() -> Option<PathBuf> {
    for key in [
        "EVERTYDESK_NV_CODEC_SDK",
        "NV_CODEC_SDK",
        "NVIDIA_VIDEO_CODEC_SDK",
    ] {
        if let Some(path) = env::var_os(key).map(PathBuf::from) {
            if is_nv_codec_sdk(&path) {
                return Some(path);
            }
        }
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR")?);
    let entries = std::fs::read_dir(&manifest_dir).ok()?;
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("Video_Codec_SDK_") && is_nv_codec_sdk(&path) {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates.pop()
}

fn is_nv_codec_sdk(path: &PathBuf) -> bool {
    path.join("Interface").join("nvEncodeAPI.h").is_file()
}

fn sdk_version(path: &PathBuf) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("Video_Codec_SDK_").map(str::to_owned)
}

fn compile_nvenc_windows_shim(sdk: &PathBuf) {
    println!("cargo:rerun-if-changed=src/nvenc_shim.cpp");
    println!("cargo:rerun-if-changed={}", sdk.join("Interface").display());
    println!("cargo:rustc-link-lib=d3d11");
    println!("cargo:rustc-link-lib=dxgi");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("src/nvenc_shim.cpp")
        .include(sdk.join("Interface"));
    build.flag_if_supported("/std:c++17");
    build.flag_if_supported("-std=c++17");
    build.compile("everty_nvenc_shim");
}
