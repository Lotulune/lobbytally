fn main() {
    // Rebuild when the release stamp changes so /v1/meta.build_git_sha stays accurate.
    println!("cargo:rerun-if-env-changed=MPGS_BUILD_GIT_SHA");
    let sha = std::env::var("MPGS_BUILD_GIT_SHA").unwrap_or_else(|_| "unknown".to_owned());
    // Avoid breaking the compiler env with newlines or quotes from odd shells.
    let sha = sha.lines().next().unwrap_or("unknown").trim();
    let sha = if sha.is_empty() { "unknown" } else { sha };
    println!("cargo:rustc-env=MPGS_BUILD_GIT_SHA={sha}");
}
