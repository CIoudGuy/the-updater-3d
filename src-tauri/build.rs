fn main() {
    // Release settings, kept in their own files so forks can point at their own feed:
    //   .update-feed  address of the app's own release feed
    //   .update-key   key for that feed when it is private
    // Environment variables with the same meaning win over the files.
    for (file, var) in [(".update-feed", "UPDATER3D_FEED_URL"), (".update-key", "UPDATER3D_UPDATE_KEY")] {
        println!("cargo:rerun-if-changed={file}");
        println!("cargo:rerun-if-env-changed={var}");
        if std::env::var(var).is_err() {
            if let Ok(v) = std::fs::read_to_string(file) {
                let v = v.trim();
                if !v.is_empty() {
                    println!("cargo:rustc-env={var}={v}");
                }
            }
        }
    }
    tauri_build::build()
}
