use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bindings");

    if !out_path.exists() {
        std::fs::create_dir(&out_path).unwrap();
    }

    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if os == "macos" || os == "ios" || os == "watchos" || os == "tvos" {
        let bindings = bindgen::Builder::default()
            .header_contents("wrapper.h", "#include <sys/sysctl.h>")
            .parse_callbacks(Box::new(bindgen::CargoCallbacks))
            .ctypes_prefix("::libc")
            .use_core()
            .generate()
            .expect("Unable to generate bindings");

        bindings
            .write_to_file(out_path.join("sysctl.rs"))
            .expect("Couldn't write bindings!");
    }
}
