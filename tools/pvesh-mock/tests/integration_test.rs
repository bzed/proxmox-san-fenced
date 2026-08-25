use std::process::Command;

#[test]
fn test_pvesh_mock_runs() {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let bin = path.join("pvesh-mock");
    
    let output = Command::new(bin)
        .arg("--help")
        .output()
        .expect("Failed to run pvesh-mock");
    
    assert!(output.status.success());
}
