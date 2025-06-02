use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::{path, vec};

use log::trace;
use serde::Deserialize;
use tempfile::NamedTempFile;

use crate::ToConfig;
use crate::errors::{DataSourceError, TaskError};
use crate::network::rpc_message::SourceType;
use crate::sources::controllers::Controller;
use crate::sources::{BUFSIZE, DataMsg, DataSourceConfig, DataSourceT};

/// Config struct which can be parsed from a toml config
#[derive(Debug, Deserialize, Clone)]
pub struct CsvConfig {
    /// Path to the CSV file
    pub path: path::PathBuf,
    /// Cell delimiter used in the CSV file
    pub cell_delimiter: u8,
    /// Row delimiter used in the CSV file
    pub row_delimiter: u8,
    /// Header row in the CSV file
    pub header: bool,
    /// The delay between reads
    pub delay: u32,
}

pub struct CsvSource {
    config: CsvConfig,
    file: File,
    reader: BufReader<File>,
    buffer: Vec<u8>,
}

impl CsvSource {
    pub fn new(config: CsvConfig) -> Result<Self, DataSourceError> {
        trace!("Creating new CSV source (path: {})", config.path.display());
        let file = File::open(&config.path)
            .map_err(|e| DataSourceError::GenericError(format!("Failed to open CSV file: {}: {}", config.path.display(), e)))?;
        let mut reader = BufReader::new(
            file.try_clone()
                .map_err(|e| DataSourceError::GenericError(format!("Failed to clone CSV file: {}: {}", config.path.display(), e)))?,
        );
        let mut buffer = vec![0; 8192];

        if config.header {
            reader
                .read_until(config.row_delimiter, &mut Vec::new())
                .map_err(|e| DataSourceError::GenericError(format!("Failed to read header from CSV file: {}: {}", config.path.display(), e)))?;
        }
        Ok(Self {
            config,
            file,
            reader,
            buffer,
        })
    }
}

/// Source implementation
#[async_trait::async_trait]
impl DataSourceT for CsvSource {
    /// Read data from source
    /// ---------------------
    /// Copy one "packet" (meaning being source specific) into the buffer and report
    /// its size.
    async fn read_buf(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
        // create str buff
        let mut line: &mut Vec<u8> = &mut Vec::new();
        // read line from file
        let bytes_read = self
            .reader
            .read_until(self.config.row_delimiter, line)
            .map_err(|e| DataSourceError::GenericError(format!("Failed to read from CSV file: {}: {}", self.config.path.display(), e)))?;
        // put the line into the buffer
        buf[..bytes_read].copy_from_slice(line);
        // sleep for the delay
        tokio::time::sleep(tokio::time::Duration::from_millis(self.config.delay.into())).await;
        Ok(bytes_read)
    }

    /// Start the data source
    /// -------------------
    /// Start the data source and prepare it for reading data. This may involve
    /// opening files, establishing network connections, etc.
    async fn start(&mut self) -> Result<(), DataSourceError> {
        trace!("Starting CSV source");
        Ok(())
    }
    /// Stop the data source
    /// ----------------
    /// Stop the data source and release any resources it holds. This may involve
    /// closing files, terminating network connections, etc.
    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!("Stopping CSV source");
        Ok(())
    }

    async fn read(&mut self) -> Result<Option<DataMsg>, DataSourceError> {
        let mut temp_buf = vec![0u8; BUFSIZE];
        match self.read_buf(&mut temp_buf).await? {
            0 => Ok(None),
            n => Ok(Some(DataMsg::RawFrame {
                ts: chrono::Utc::now().timestamp_millis() as f64 / 1e3,
                bytes: temp_buf[..n].to_vec(),
                source_type: SourceType::CSV,
            })),
        }
    }
}

#[async_trait::async_trait]
impl ToConfig<DataSourceConfig> for CsvSource {
    /// Attempts to convert the current `CsvSource` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `CsvSource`, but it is currently not
    /// implemented and always returns an error. This serves as a placeholder for future
    /// support, where configuration export for `CsvSource` may be needed.
    ///
    /// # Returns
    /// - `Err(TaskError::NotImplemented)` to indicate that this functionality is not available.
    async fn to_config(&self) -> Result<DataSourceConfig, TaskError> {
        Err(TaskError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_csv_source_new_success() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2").unwrap();

        let config = CsvConfig {
            path: temp_file.path().to_path_buf(),
            cell_delimiter: b',',
            row_delimiter: b'\n',
            header: true,
            delay: 1,
        };

        let csv_source = CsvSource::new(config.clone());
        assert!(csv_source.is_ok());
    }

    #[tokio::test]
    async fn test_csv_source_new_file_not_found() {
        let config = CsvConfig {
            path: "non_existent_file.csv".into(),
            cell_delimiter: b',',
            row_delimiter: b'\n',
            header: true,
            delay: 1,
        };

        let csv_source = CsvSource::new(config);
        assert!(csv_source.is_err());
    }

    #[tokio::test]
    async fn test_csv_source_read_success() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2").unwrap();

        let config = CsvConfig {
            path: temp_file.path().to_path_buf(),
            cell_delimiter: b',',
            row_delimiter: b'\n',
            header: true,
            delay: 1,
        };

        let mut csv_source = CsvSource::new(config).unwrap();
        let mut buffer = vec![0; 1024];
        let bytes_read = csv_source.read_buf(&mut buffer).await.unwrap();
        assert!(bytes_read > 0);
    }

    #[tokio::test]
    async fn test_csv_source_start_and_stop() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2").unwrap();

        let config = CsvConfig {
            path: temp_file.path().to_path_buf(),
            cell_delimiter: b',',
            row_delimiter: b'\n',
            header: true,
            delay: 1,
        };

        let mut csv_source = CsvSource::new(config).unwrap();
        assert!(csv_source.start().await.is_ok());
        assert!(csv_source.stop().await.is_ok());
    }
}
