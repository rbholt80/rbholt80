//! Link against the system's X11, Cairo and Pango.
//!
//! These are the libraries every Linux desktop already has loaded -- Cinnamon,
//! GNOME and XFCE all draw with Cairo and shape text with Pango. Binding to
//! them directly keeps NOUS dependency-free in the Rust sense while using the
//! same text stack as every other native application on the machine.

fn main() {
    // pkg-config is the portable way to find these; fall back to plain names
    // if it is absent, which covers minimal build environments.
    let libs = ["x11", "cairo", "pangocairo", "gobject-2.0"];
    let mut ok = true;
    for lib in libs {
        let out = std::process::Command::new("pkg-config")
            .args(["--libs", lib])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                for token in String::from_utf8_lossy(&o.stdout).split_whitespace() {
                    if let Some(name) = token.strip_prefix("-l") {
                        println!("cargo:rustc-link-lib={}", name);
                    } else if let Some(path) = token.strip_prefix("-L") {
                        println!("cargo:rustc-link-search=native={}", path);
                    }
                }
            }
            _ => ok = false,
        }
    }
    if !ok {
        for lib in [
            "X11",
            "cairo",
            "pangocairo-1.0",
            "pango-1.0",
            "gobject-2.0",
            "glib-2.0",
        ] {
            println!("cargo:rustc-link-lib={}", lib);
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
