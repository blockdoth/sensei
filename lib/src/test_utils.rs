use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

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
        .arg("10000")
        .status()
        .expect("Failed to execute data generation script");
    assert!(status.success(), "Python script did not run successfully");

    // Print the contents of the generated file for debugging
    let mut contents = String::new();
    File::open(file.path()).unwrap().read_to_string(&mut contents).unwrap();
    trace!("Generated CSV file contents:\n{contents}");

    file
}

// Dummy packet, used for testing the iwl5300
pub fn build_test_packet(
    code: u8,
    sequence_number: u16,
    nrx_ntx: [u8; 2],
    rssi: [u8; 3],
    noise: i8,
    agc_antenna_sel: [u8; 2],
    csi_len_override: Option<usize>,
) -> Vec<u8> {
    let mut buf = vec![];
    buf.push(code);
    buf.extend(&0u32.to_le_bytes());
    buf.extend(&0u16.to_le_bytes());
    buf.extend(&sequence_number.to_le_bytes());
    buf.push(nrx_ntx[0]);
    buf.push(nrx_ntx[1]);
    buf.extend(&rssi);
    buf.push(noise as u8);
    buf.push(agc_antenna_sel[0]);
    buf.push(agc_antenna_sel[1]);
    let nrx_usize = nrx_ntx[0] as usize;
    let ntx_usize = nrx_ntx[1] as usize;
    let csi_len = ((30 * (nrx_usize * ntx_usize * 8 * 2 + 3)) / 8) + 1;
    let len = csi_len_override.unwrap_or(csi_len);
    buf.extend(&(len as u16).to_le_bytes());

    while buf.len() < 21 {
        buf.push(0);
    }
    buf.extend(vec![0xAB; len]);

    buf
}
