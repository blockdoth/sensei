use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

use lib::handler::device_handler::DeviceHandlerConfig;
use log::LevelFilter;
use serde::Deserialize;

use crate::system_node::SinkConfigWithName;

pub const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

/// A trait for parsing a YAML file into a struct using Serde.
///
/// This trait provides a generic method to deserialize a YAML file into any type that implements
/// the `Deserialize` trait from Serde. The method reads the contents of the specified file path,
/// and attempts to parse it into the desired type.
///
/// # Methods
/// - `fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error>>`: Reads the YAML file at the given
///   path and deserializes it into the implementing type.
///
/// # Errors
/// Returns a `Box<dyn std::error::Error>` if the file cannot be read or if deserialization fails.
///
/// # Example
/// ```rust,ignore
/// let config: MyConfig = MyTrait::from_yaml(PathBuf::from("config.yaml"))?;
/// ```
pub trait FromYaml: Sized + for<'de> Deserialize<'de> {
    /// Loads an instance of the implementing type from a YAML file.
    ///
    /// # Arguments
    ///
    /// * `file` - The path to the YAML file to be read.
    ///
    /// # Returns
    ///
    /// * `Ok(Self)` if deserialization is successful.
    /// * `Err(Box<dyn std::error::Error>)` if reading or deserialization fails.
    ///
    /// # Panics
    ///
    /// This function will panic if the file cannot be read.
    fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

pub struct OrchestratorConfig {
    pub experiment_config: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SystemNodeConfig {
    pub addr: SocketAddr,
    pub host_id: u64,
    pub registries: Option<Vec<SocketAddr>>,
    pub registry_polling_rate_s: Option<u64>,
    pub device_configs: Vec<DeviceHandlerConfig>,
    #[serde(default)]
    pub sinks: Vec<SinkConfigWithName>,
}

pub struct VisualiserConfig {
    pub target: SocketAddr,
    pub ui_type: String,
}

pub struct EspToolConfig {
    pub serial_port: String,
}

pub struct GlobalConfig {
    pub log_level: LevelFilter,
}

pub enum ServiceConfig {
    Orchestrator(OrchestratorConfig),
    SystemNode(SystemNodeConfig),
    Visualiser(VisualiserConfig),
    EspTool(EspToolConfig),
}

pub trait Run<ServiceConfig> {
    // Initialize standalone state which does not depend on any config
    fn new(global_config: GlobalConfig, config: ServiceConfig) -> Self;

    // Actually applies given config and runs the service
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>>;
}

impl FromYaml for SystemNodeConfig {}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    #[derive(Deserialize, PartialEq, Debug)]
    struct TestConfig {
        field1: String,
        field2: i32,
    }

    impl FromYaml for TestConfig {}

    #[test]
    fn test_from_yaml_success() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_config.yaml");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "field1: hello\nfield2: 123").unwrap();

        let config = TestConfig::from_yaml(file_path).unwrap();
        assert_eq!(
            config,
            TestConfig {
                field1: "hello".to_string(),
                field2: 123
            }
        );
    }

    #[test]
    fn test_from_yaml_file_not_found() {
        let result = TestConfig::from_yaml(PathBuf::from("non_existent_file.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_from_yaml_malformed_content() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("malformed_config.yaml");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "field1: hello\\nfield2: not_an_integer").unwrap();

        let result = TestConfig::from_yaml(file_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_system_node_config_from_yaml() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("system_node_config.yaml");
        let mut file = File::create(&file_path).unwrap();
        writeln!(
            file,
            r#"
addr: "127.0.0.1:8080"
host_id: 1
device_configs: []
"#
        )
        .unwrap();

        let config = SystemNodeConfig::from_yaml(file_path).unwrap();
        assert_eq!(config.addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.host_id, 1);
        assert!(config.registries.is_none()); // Ensure default is handled
    }
}
