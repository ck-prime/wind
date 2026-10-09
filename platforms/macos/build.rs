//! Point the linker at the Swift compatibility libraries.
//!
//! `screencapturekit` looks under `XcodeDefault.xctoolchain`. A machine with
//! only the Command Line Tools keeps those libraries under `usr/lib/swift/macosx`.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    let Ok(output) = Command::new("xcode-select").arg("-p").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let Ok(developer_dir) = String::from_utf8(output.stdout) else {
        return;
    };
    let developer_dir = developer_dir.trim();
    let candidates = [
        format!("{developer_dir}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx"),
        format!("{developer_dir}/usr/lib/swift/macosx"),
    ];
    for candidate in candidates {
        if Path::new(&candidate)
            .join("libswiftCompatibility56.a")
            .is_file()
        {
            println!("cargo:rustc-link-search=native={candidate}");
            return;
        }
    }
}
