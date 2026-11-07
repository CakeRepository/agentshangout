//! CR11: My bots shows each bot's car and the Garage sells cars, run from tests/garage_ui.cjs.
#[test]
fn garage_ui() {
    let output =
        std::process::Command::new("node").arg("tests/garage_ui.cjs").output().expect("node runs");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
