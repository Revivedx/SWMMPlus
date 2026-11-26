// UK_MOD_VERSION: the version of uk-mod this build packages and reads mods with, from the workspace's Cargo.lock. A
// mod's meta.yml carries it as `api`, and uk-mod refuses mods whose `api` has another major version.
fn main() {
    let lock_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    let lock = std::fs::read_to_string(&lock_path).expect("Cargo.lock");
    let version = lock
        .split("[[package]]")
        .find(|p| p.lines().any(|l| l.trim() == "name = \"uk-mod\""))
        .and_then(|p| p.lines().find_map(|l| l.trim().strip_prefix("version = \"")))
        .and_then(|v| v.strip_suffix('"'))
        .expect("uk-mod in Cargo.lock");
    println!("cargo:rustc-env=UK_MOD_VERSION={version}");
}
