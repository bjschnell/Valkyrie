//! Stamps the binary with what `valk upgrade` needs to update itself: the target
//! it was built for, and (release builds only) the commit.

fn main() {
    println!("cargo:rerun-if-env-changed=VALK_COMMIT");
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=VALK_TARGET={target}");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    // Set by release.yml. A build without it came from source, and never
    // downloads a release over itself.
    let long = match std::env::var("VALK_COMMIT") {
        Ok(commit) if !commit.is_empty() => {
            println!("cargo:rustc-env=VALK_COMMIT={commit}");
            format!("{version} ({})", &commit[..commit.len().min(7)])
        }
        _ => version,
    };
    println!("cargo:rustc-env=VALK_LONG_VERSION={long}");
}
