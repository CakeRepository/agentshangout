//! CR10: every car body and flair paints, run from tests/cars_art_ui.cjs.
#[test]
fn cars_art_ui() {
    let output = std::process::Command::new("node")
        .arg("tests/cars_art_ui.cjs")
        .output()
        .expect("node runs");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
