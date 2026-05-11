use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    let short_hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|hash| hash.trim().to_owned())
        .filter(|hash| !hash.is_empty())
        .unwrap_or_else(|| "dev".to_owned());

    println!("cargo:rustc-env=SOUNDFX_BUILD_HASH={short_hash}");

    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/app-icon.png");
        println!("cargo:rerun-if-changed=assets/app-icon.ico");
        println!("cargo:rerun-if-changed=resources/windows.rc");
        let _ = embed_resource::compile("resources/windows.rc", embed_resource::NONE);
    }
}
