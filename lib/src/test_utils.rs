use std::{fs::File, io::Read, path::PathBuf, process::Command};

use log::trace;
use tempfile::NamedTempFile;

const DEFAULT_SCRIPTS_PATH: &str = "./scripts";
const BACKUP_SCRIPTS_PATH: &str = "../scripts";

pub fn generate_csv_data_file() -> NamedTempFile {
    let scripts_path = if PathBuf::from(DEFAULT_SCRIPTS_PATH).exists() {
        PathBuf::from(DEFAULT_SCRIPTS_PATH)
    } else if PathBuf::from(BACKUP_SCRIPTS_PATH).exists() {
        PathBuf::from(BACKUP_SCRIPTS_PATH)
    } else {
        panic!("Could not find a resource path")
    };
    let file = NamedTempFile::new().unwrap();

    // Since the nix environment specifies that we have access to python3, we can use it in a test :)
    // Execute the Python script to generate random data
    let script_path = scripts_path.join("generate_data.py");
    let status = Command::new("python3")
        .arg(script_path)
        .arg(file.path())
        .arg("10".to_string())
        .status()
        .expect("Failed to execute data generation script");
    assert!(status.success(), "Python script did not run successfully");

    // Print the contents of the generated file for debugging
    let mut contents = String::new();
    File::open(file.path())
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    trace!("Generated CSV file contents:\n{}", contents);

    file
}
