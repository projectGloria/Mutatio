// Compiles mutatio.rc (icon + VERSIONINFO) into the executable via the Windows SDK's rc.exe.
// The .rc is a template; values are substituted here rather than passed as /D defines, because
// rc.exe mangles quoted string defines coming through the command line.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=mutatio.rc");
    println!("cargo:rerun-if-changed=Mutatio.ico");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();

    let mut nums = version.split(['.', '-']).map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0), nums.next().unwrap_or(0));

    // rc string literals use C escaping, so backslashes in the absolute path must be doubled.
    let icon = manifest_dir.join("Mutatio.ico").display().to_string().replace('\\', "\\\\");

    let rc = std::fs::read_to_string(manifest_dir.join("mutatio.rc"))
        .unwrap()
        .replace("@ICON_PATH@", &icon)
        .replace("@VER_COMMA@", &format!("{major}, {minor}, {patch}, 0"))
        .replace("@VER_STR@", &version);

    let generated = out_dir.join("mutatio.rc");
    std::fs::write(&generated, rc).unwrap();

    embed_resource::compile(&generated, embed_resource::NONE)
        .manifest_optional()
        .expect("failed to compile Windows resources (is the Windows SDK installed?)");
}
