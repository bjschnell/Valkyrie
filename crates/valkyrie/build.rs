//! Stamps the binary with what `valk upgrade` needs to update itself: the target
//! it was built for, and (release builds only) the commit and the channel.

fn main() {
    println!("cargo:rerun-if-env-changed=VALK_COMMIT");
    println!("cargo:rerun-if-env-changed=VALK_CHANNEL");
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=VALK_TARGET={target}");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    // Set by build.yml: `edge` for the rolling pre-release (edge.yml), empty for a
    // versioned one.
    let channel = std::env::var("VALK_CHANNEL").unwrap_or_default();
    if !channel.is_empty() {
        println!("cargo:rustc-env=VALK_CHANNEL={channel}");
    }
    // Set by build.yml. A build without it came from source, and never
    // downloads a release over itself.
    let long = match std::env::var("VALK_COMMIT") {
        Ok(commit) if !commit.is_empty() => {
            println!("cargo:rustc-env=VALK_COMMIT={commit}");
            let short = &commit[..commit.len().min(7)];
            match channel.as_str() {
                "" => format!("{version} ({short})"),
                channel => format!("{version} ({channel} {short})"),
            }
        }
        _ => version,
    };
    println!("cargo:rustc-env=VALK_LONG_VERSION={long}");
}
