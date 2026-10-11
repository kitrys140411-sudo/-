//! POSIX scenario tests: every tests/posix/*.sh is run with lsh and its stdout
//! compared with the matching .out file (recorded from dash, the reference).

use std::path::Path;
use std::process::Command;

#[test]
fn posix_scenarios() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/posix");
    let mut scripts: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sh"))
        .collect();
    scripts.sort();
    assert!(!scripts.is_empty());
    let mut failures = Vec::new();
    for script in &scripts {
        let expected = std::fs::read_to_string(script.with_extension("out")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_lsh"))
            .arg(script)
            .env("HOME", "/root")
            .env_remove("HOME_LIKE")
            .current_dir("/")
            .output()
            .unwrap();
        let got = String::from_utf8_lossy(&out.stdout);
        if got != expected {
            failures.push(format!(
                "{}:\n--- expected\n{expected}--- got\n{got}--- stderr\n{}",
                script.display(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), scripts.len(), failures.join("\n"));
}
