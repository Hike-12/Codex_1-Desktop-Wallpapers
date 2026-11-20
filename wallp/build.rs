use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=Info.plist");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=stilltide.rc");
        println!("cargo:rerun-if-changed=../branding/stilltide.ico");
        embed_resource::compile("stilltide.rc", embed_resource::NONE)
            .manifest_required()
            .expect("compile Stilltide's Windows icon");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let plist = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
        .join("Info.plist");
    for part in [
        "-sectcreate",
        "__TEXT",
        "__info_plist",
        &plist.to_string_lossy(),
    ] {
        println!("cargo:rustc-link-arg=-Xlinker");
        println!("cargo:rustc-link-arg={part}");
    }
}
