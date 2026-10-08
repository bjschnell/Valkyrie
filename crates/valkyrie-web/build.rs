// The app is embedded from web/dist; rebuild when it changes.
fn main() {
    println!("cargo:rerun-if-changed=../../web/dist");
}
