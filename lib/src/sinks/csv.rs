//! The csv sink is functionally pretty much a wrapper around the file sink,
//! but before writing a csi data frame, it converst it to a format that can be interpreted by the CSV adapter.
//!
//! The default Csi data format of
//! CsiData { timestamp, sequence_numberm, rssi, csi }
//!
//! Get converted to :
//! timestamp,sequence_number,num_cores,num_streams,num_subcarriers,rssi,csi
//! example:
//! 3418319.67224585,23074,2,1,1,"98,28","(-0.9306254246008658-0.7152900288177244j),(-0.9533073248869779-0.8846499863951724j)"

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use async_trait::async_trait;

use crate::csi_types::{Complex, CsiData};
use crate::errors::{SinkError, TaskError};
use crate::network::rpc_message::DataMsg;
use crate::sinks::{Sink, SinkConfig, ToConfig};
pub struct CSVSink {
    writer: BufWriter<File>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct CSVConfig {
    path: PathBuf,
}

impl CSVSink {
    pub async fn new(config: CSVConfig) -> Result<Self, SinkError> {
        let file = File::create(config.path)?;
        let mut writer = BufWriter::new(file);
        // Write header
        writeln!(writer, "timestamp,sequence_number,num_cores,num_streams,num_subcarriers,rssi,csi")?;
        Ok(CSVSink { writer })
    }

    fn write(&mut self, data: &CsiData) -> Result<(), SinkError> {
        let num_cores = data.csi.len();
        let num_antennas = if num_cores > 0 { data.csi[0].len() } else { 0 };
        let num_subcarriers = if num_cores > 0 && num_antennas > 0 { data.csi[0][0].len() } else { 0 };

        // rssi as CSV string
        let rssi_str = data.rssi.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",");

        // csi as CSV string: flatten all (real, imag) pairs
        let mut csi_vec = Vec::new();
        for core in &data.csi {
            for antenna in core {
                for &Complex { re, im } in antenna {
                    csi_vec.push(format!("({re}|{im}j)"));
                }
            }
        }
        let csi_str = csi_vec.join(",");

        writeln!(
            self.writer,
            "{},{},{},{},{},\"{}\",\"{}\"",
            data.timestamp, data.sequence_number, num_cores, num_antennas, num_subcarriers, rssi_str, csi_str
        )?;
        self.flush();
        Ok(())
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        match self.writer.flush() {
            Ok(_) => Ok(()),
            Err(e) => Err(SinkError::Io(e)),
        }
    }
}

#[async_trait]
impl ToConfig<SinkConfig> for CSVSink {
    async fn to_config(&self) -> Result<SinkConfig, TaskError> {
        // You may need to adjust this depending on your SinkConfig definition
        Ok(SinkConfig::CSV(CSVConfig { path: todo!() }))
    }
}

#[async_trait]
impl Sink for CSVSink {
    /// Open the connection to the sink
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn open(&mut self) -> Result<(), SinkError> {
        Ok(())
    }

    /// Closes the connection to the sink
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn close(&mut self) -> Result<(), SinkError> {
        Ok(())
    }

    /// Consume a DataMsg and process it (e.g., write to file, send over network).
    /// Consume a [`DataMsg`] and perform a sink-specific operation.
    ///
    /// # Errors
    ///
    /// Returns a [`SinkError`] if the operation fails (e.g., I/O failure).
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError> {
        match data {
            DataMsg::CsiFrame { csi } => {
                self.write(&csi)?;
                Ok(())
            }
            DataMsg::RawFrame { .. } => {
                // Optionally: return an error or just ignore
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{File, remove_file};
    use std::io::{BufRead, BufReader, Read, Seek};

    use log::error;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::adapters::CsiDataAdapter;
    use crate::adapters::csv::CSVAdapter;
    use crate::network::rpc_message::{DataMsg, SourceType};
    use crate::sources::DataSourceT;
    use crate::sources::csv::{CsvConfig, CsvSource};
    use crate::test_utils;

    fn dummy_csi_data() -> CsiData {
        CsiData {
            timestamp: 1234567.89,
            sequence_number: 42,
            rssi: vec![99, 100],
            csi: vec![
                vec![vec![Complex { re: 1.0, im: -2.0 }, Complex { re: 3.5, im: 4.5 }]],
                vec![vec![Complex { re: -0.1, im: 0.2 }, Complex { re: 0.0, im: 0.0 }]],
            ],
        }
    }

    #[tokio::test]
    async fn test_csvsink_write_and_flush() {
        let path = "test_output.csv";
        let mut sink = CSVSink::new(CSVConfig { path: path.into() }).await.unwrap();
        let data = dummy_csi_data();
        sink.write(&data).unwrap();
        sink.flush().unwrap();

        let file = File::open(path).unwrap();
        let mut lines = BufReader::new(file).lines();

        // Check header
        let header = lines.next().unwrap().unwrap();
        assert_eq!(header, "timestamp,sequence_number,num_cores,num_streams,num_subcarriers,rssi,csi");

        // Check data line
        let line = lines.next().unwrap().unwrap();
        assert!(line.starts_with("1234567.89,42,2,1,2,\"99,100\",\"(1|-2j),(3.5|4.5j),(-0.1|0.2j),(0|0j)\""));

        remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_parse_read_adapt_and_compare() {
        let mut file = NamedTempFile::new().unwrap();
        let csv_file_option = test_utils::generate_csv_data_file();
        if csv_file_option.is_none() {
            error!("Skipped test, could not generate a CSV file");
            return;
        }
        let mut csv_file = csv_file_option.unwrap();
        let path = csv_file.path();
        let mut source = CsvSource::new(CsvConfig {
            path: (*path).to_path_buf(),
            cell_delimiter: b',',
            row_delimiter: b'\n',
            header: true,
            delay: 0,
        })
        .unwrap();
        let mut sink = CSVSink::new(CSVConfig { path: file.path().into() }).await.unwrap();
        let mut adapter = CSVAdapter::default();
        loop {
            let mut buf = vec![0u8; 128];
            let size = source.read_buf(&mut buf).await.unwrap();
            if size == 0 {
                break;
            }

            let raw_data_message = DataMsg::RawFrame {
                ts: 0.0,
                bytes: buf,
                source_type: SourceType::CSV,
            };
            // Might not return a data packet
            if let Some(data) = adapter.produce(raw_data_message).await.unwrap() {
                sink.provide(data).await;
            }
        }
        sink.flush().unwrap();

        // After writing all data, compare the contents of the temp file and the original CSV data file

        // Rewind the tempfile to read from the beginning
        let mut written_file = File::open(file.path()).unwrap();
        let mut written_contents = String::new();
        written_file.read_to_string(&mut written_contents).unwrap();

        csv_file.rewind();
        let mut csv_contents = String::new();
        csv_file.read_to_string(&mut csv_contents).unwrap();

        // Optionally, normalize line endings for comparison
        let normalize = |s: &str| s.replace("\r\n", "\n");
        let binding = normalize(&written_contents);
        let written_lines: Vec<_> = binding.lines().collect();
        let binding = normalize(&csv_contents);
        let original_lines: Vec<_> = binding.lines().collect();

        assert_eq!(
            written_lines.len(),
            original_lines.len(),
            "CSV sink output and original data have different number of lines"
        );

        for (i, (written, original)) in written_lines.iter().zip(original_lines.iter()).enumerate() {
            // Sometimes the flp to string conversion yields slightly different strings. Especially once scientific notation is used.
            // Therefore this test is kinda weird and tires to see if the contents of the string at the very least mean the same things.
            // Since comparing String to String is faster than parsing every string, that's checked first.
            if written != original {
                let split_written = adapter.parse_row(written.as_bytes()).unwrap();
                let split_original = adapter.parse_row(original.as_bytes()).unwrap();

                let written_csi = CSVAdapter::row_to_csi(split_written).unwrap();
                let original_csi = CSVAdapter::row_to_csi(split_original).unwrap();
                assert_eq!(written_csi, original_csi);
            }
        }
    }
}
