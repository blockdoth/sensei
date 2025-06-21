use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::{path, vec};

use log::trace;
use serde::{Deserialize, Serialize};

use crate::ToConfig;
use crate::errors::{DataSourceError, TaskError};
use crate::network::rpc_message::SourceType;
use crate::sources::{BUFSIZE, DataMsg, DataSourceConfig, DataSourceT};

const DEFAULT_ROW_DELIM: u8 = b'\n';
const DEFAULT_CELL_DELIM: u8 = b',';

/// Config struct which can be parsed from a toml config
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CsvSourceConfig {
    /// Path to the Csv file
    pub path: path::PathBuf,
    /// Cell delimiter used in the Csv file
    pub cell_delimiter: Option<u8>,
    /// Row delimiter used in the Csv file
    pub row_delimiter: Option<u8>,
    /// Header row in the Csv file
    pub header: bool,
    /// The delay between reads
    pub delay: u32,
}

pub struct CsvSource {
    config: CsvSourceConfig,
    reader: BufReader<File>,
    cell_delimiter: u8,
    row_delimiter: u8,
}

impl CsvSource {
    pub fn new(config: CsvSourceConfig) -> Result<Self, DataSourceError> {
        let cell_delimiter = config.cell_delimiter.unwrap_or(DEFAULT_CELL_DELIM);
        let row_delimiter = config.row_delimiter.unwrap_or(DEFAULT_ROW_DELIM);
        trace!("Creating new Csv source (path: {})", config.path.display());
        let file = File::open(&config.path)
            .map_err(|e| DataSourceError::GenericError(format!("Failed to open Csv file: {}: {}", config.path.display(), e)))?;
        let mut reader = BufReader::new(
            file.try_clone()
                .map_err(|e| DataSourceError::GenericError(format!("Failed to clone Csv file: {}: {}", config.path.display(), e)))?,
        );

        if config.header {
            reader
                .read_until(row_delimiter, &mut Vec::new())
                .map_err(|e| DataSourceError::GenericError(format!("Failed to read header from Csv file: {}: {}", config.path.display(), e)))?;
        }
        Ok(Self {
            config,
            reader,
            cell_delimiter,
            row_delimiter,
        })
    }
}

/// Source implementation
#[async_trait::async_trait]
impl DataSourceT for CsvSource {
    /// Read data from source
    /// ---------------------
    /// This function reads either the full buffer, or until a newline character is detected.
    async fn read_buf(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
        let row_delim = self.row_delimiter;
        let max_read = buf.len();
        let mut temp_buf = vec![0u8; max_read];

        let bytes_read = self.reader.read(&mut temp_buf)?;

        for i in (0..bytes_read).rev() {
            if temp_buf[i] == row_delim {
                self.reader.seek(SeekFrom::Current(i as i64 - (bytes_read - 1) as i64))?; // rewind until i, account for endl.
                buf[..(i + 1)].copy_from_slice(&temp_buf[..(i + 1)]); // +1 nonsense because of index/length variance.
                return Ok(i + 1); // account for index/length difference.
            }
        }
        buf[..bytes_read].copy_from_slice(&temp_buf[..bytes_read]);
        // sleep to simulate a delay between arriving packets.
        if self.config.delay > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(self.config.delay.into())).await;
        }
        Ok(bytes_read)
    }

    /// Start the data source
    /// -------------------
    /// Start the data source and prepare it for reading data. This may involve
    /// opening files, establishing network connections, etc.
    async fn start(&mut self) -> Result<(), DataSourceError> {
        trace!("Starting Csv source");
        Ok(())
    }
    /// Stop the data source
    /// ----------------
    /// Stop the data source and release any resources it holds. This may involve
    /// closing files, terminating network connections, etc.
    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!("Stopping Csv source");
        Ok(())
    }

    async fn read(&mut self) -> Result<Option<DataMsg>, DataSourceError> {
        let mut temp_buf = vec![0u8; BUFSIZE];
        match self.read_buf(&mut temp_buf).await? {
            0 => Ok(None),
            n => Ok(Some(DataMsg::RawFrame {
                ts: chrono::Utc::now().timestamp_millis() as f64 / 1e3,
                bytes: temp_buf[..n].to_vec(),
                source_type: SourceType::Csv,
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
    use std::io::Write;
    use std::path::PathBuf;

    use tempfile::NamedTempFile;

    use super::*;

    fn get_csv_conf(path: PathBuf) -> CsvSourceConfig {
        CsvSourceConfig {
            path,
            cell_delimiter: Some(DEFAULT_CELL_DELIM),
            row_delimiter: Some(DEFAULT_ROW_DELIM),
            header: true,
            delay: 1,
        }
    }

    #[tokio::test]
    async fn test_csv_source_read_buf_partial_line() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2\nvalue3,value4").unwrap();

        let config = get_csv_conf(temp_file.path().to_path_buf());

        let mut csv_source = CsvSource::new(config).unwrap();

        // Buffer smaller than a full line ("value1,value2\n" is 13 bytes)
        let mut buffer = vec![0; 5];
        let bytes_read = csv_source.read_buf(&mut buffer).await.unwrap();
        assert_eq!(bytes_read, 5);
        // Should only contain the first 5 bytes of the first data line
        assert_eq!(&buffer[..bytes_read], b"value");

        // Read the rest of the line
        let mut buffer2 = vec![0; 16];
        let bytes_read2 = csv_source.read_buf(&mut buffer2).await.unwrap();
        assert!(bytes_read2 > 0);
        // Should contain the rest of the first line (including delimiter) or start of next line
        assert!(
            std::str::from_utf8(&buffer2[..bytes_read2]).unwrap().contains("1,value2\n")
                || std::str::from_utf8(&buffer2[..bytes_read2]).unwrap().contains("1,value2")
        );
    }

    #[tokio::test]
    async fn test_csv_source_read_buf_exact_line() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2\n").unwrap();

        let config = get_csv_conf(temp_file.path().to_path_buf());

        let mut csv_source = CsvSource::new(config).unwrap();

        // "value1,value2\n" is 13 bytes
        let mut buffer = vec![0; 14];
        let bytes_read = csv_source.read_buf(&mut buffer).await.unwrap();
        assert_eq!(bytes_read, 14);
        assert_eq!(&buffer[..bytes_read], b"value1,value2\n");
    }

    #[tokio::test]
    async fn test_csv_source_read_buf_multiple_small_reads() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2\nvalue3,value4\n").unwrap();

        let config = get_csv_conf(temp_file.path().to_path_buf());

        let mut csv_source = CsvSource::new(config).unwrap();

        // Read first line in small chunks
        let mut buffer = vec![0; 4];
        let bytes_read1 = csv_source.read_buf(&mut buffer).await.unwrap();
        assert_eq!(bytes_read1, 4);

        let mut buffer2 = vec![0; 4];
        let bytes_read2 = csv_source.read_buf(&mut buffer2).await.unwrap();
        assert_eq!(bytes_read2, 4);

        let mut buffer3 = vec![0; 10];
        let bytes_read3 = csv_source.read_buf(&mut buffer3).await.unwrap();
        assert!(bytes_read3 > 0);

        // After three reads, we should have consumed at least the first line and started the second
        let mut collected = Vec::new();
        collected.extend_from_slice(&buffer[..bytes_read1]);
        collected.extend_from_slice(&buffer2[..bytes_read2]);
        collected.extend_from_slice(&buffer3[..bytes_read3]);
        let collected_str = String::from_utf8_lossy(&collected);
        assert!(collected_str.contains("value1,value2"));
    }

    #[tokio::test]
    async fn test_csv_source_read_buf_eof() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2").unwrap();

        let config = get_csv_conf(temp_file.path().to_path_buf());

        let mut csv_source = CsvSource::new(config).unwrap();

        let mut buffer = vec![0; 30];
        let bytes_read = csv_source.read_buf(&mut buffer).await.unwrap();
        assert!(bytes_read > 0);

        // Next read should return 0 (EOF)
        let bytes_read2 = csv_source.read_buf(&mut buffer).await.unwrap();
        assert_eq!(bytes_read2, 0);
    }

    #[tokio::test]
    async fn test_csv_source_new_success() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2").unwrap();

        let config = get_csv_conf(temp_file.path().to_path_buf());

        let csv_source = CsvSource::new(config.clone());
        assert!(csv_source.is_ok());
    }

    #[tokio::test]
    async fn test_csv_source_new_file_not_found() {
        let config = get_csv_conf("Nonexisting".into());

        let csv_source = CsvSource::new(config);
        assert!(csv_source.is_err());
    }

    #[tokio::test]
    async fn test_csv_source_read_success() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2").unwrap();

        let config = get_csv_conf(temp_file.path().to_path_buf());

        let mut csv_source = CsvSource::new(config).unwrap();
        let mut buffer = vec![0; 1024];
        let bytes_read = csv_source.read_buf(&mut buffer).await.unwrap();
        assert!(bytes_read > 0);
    }

    #[tokio::test]
    async fn test_csv_source_start_and_stop() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "header1,header2\nvalue1,value2").unwrap();

        let config = get_csv_conf(temp_file.path().to_path_buf());

        let mut csv_source = CsvSource::new(config).unwrap();
        assert!(csv_source.start().await.is_ok());
        assert!(csv_source.stop().await.is_ok());
    }
}
