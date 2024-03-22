fn get_git_commit_id() -> String {
    match std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg("--short")
        .arg("HEAD")
        .output()
    {
        Ok(output) => {
            let s = String::from_utf8_lossy(&output.stdout);
            s.trim().to_string()
        }
        Err(e) => {
            panic!("failed to get git commit id: {}", e);
        }
    }
}

fn main() {
    println!("cargo:rustc-env=GIT_COMMIT_ID={}", get_git_commit_id());
}
