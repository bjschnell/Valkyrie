fn main() {
    // Only the app needs Tauri's build step; the core is a plain library.
    #[cfg(feature = "window")]
    tauri_build::build();
}
