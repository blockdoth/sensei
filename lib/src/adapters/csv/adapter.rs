use std::vec;

use crate::ToConfig;
use crate::adapters::{CsiDataAdapter, DataAdapterConfig};
use crate::csi_types::{Complex, CsiData};
use crate::errors::{CSVAdapterError, CsiAdapterError, TaskError};
use crate::network::rpc_message::DataMsg;

const DEFAULT_LINE_DELIM: u8 = b'\n';
const DEFAULT_CELL_DELIM: u8 = b',';
const ROW_SIZE: usize = 7;

pub struct CSVAdapter<'a> {
    buffer: Vec<u8>,
    tmp_data: Option<CsiData>,
    cell_delimiter: &'a u8,
    line_delimiter: &'a u8,
}

#[allow(clippy::needless_lifetimes)] // TODO: fix this
/// Implementation of the `CSVAdapter` for parsing CSV-formatted CSI data.
///
/// # Methods
///
/// - `new(buffer, tmp_data, cell_delimiter, line_delimiter) -> Self`  
///   Constructs a new `CSVAdapter` with the provided buffer, optional temporary data, and optional cell and line delimiters.
///
/// - `split_rows(&mut self) -> Vec<Vec<u8>>`  
///   Splits the internal buffer into rows using the configured line delimiter, returning a vector of rows as byte vectors. The buffer is updated to contain only the unprocessed remainder.
///
/// - `parse_row(&self, row: &[u8]) -> Result<Vec<String>, CsiAdapterError>`  
///   Parses a single CSV row into a vector of cell strings, handling quoted fields. Returns an error if parsing fails or the row is empty.
///
/// - `consume(&mut self, buf: &[u8]) -> Result<Option<CsiData>, CsiAdapterError>`  
///   Consumes additional bytes into the buffer, splits and parses the last complete row, and attempts to construct a `CsiData` instance from the parsed cells. Handles parsing of numeric fields, RSSI values, and complex CSI values. Returns `Ok(Some(CsiData))` if a row was successfully parsed, `Ok(None)` if no complete row is available, or an error if parsing fails.
impl<'a> CSVAdapter<'a> {
    pub fn new(buffer: Vec<u8>, tmp_data: Option<CsiData>, cell_delimiter: Option<&'a u8>, line_delimiter: Option<&'a u8>) -> Self {
        Self {
            buffer,
            tmp_data,
            cell_delimiter: cell_delimiter.unwrap_or(&DEFAULT_CELL_DELIM),
            line_delimiter: line_delimiter.unwrap_or(&DEFAULT_LINE_DELIM),
        }
    }
    /// Splits the internal buffer into rows using the configured line delimiter,
    /// and keeps only the unprocessed remainder in the buffer.
    fn split_rows(&mut self) -> Vec<Vec<u8>> {
        let mut rows = Vec::new();
        let mut start = 0;
        for (i, &b) in self.buffer.iter().enumerate() {
            if b == *self.line_delimiter {
                rows.push(self.buffer[start..i].to_vec());
                start = i + 1;
            }
        }
        // Remove processed bytes from buffer
        self.buffer = self.buffer[start..].to_vec();
        rows
    }

    /// Parses a single CSV row into a vector of cells, handling quoted fields.
    ///
    /// # Errors
    ///
    /// This function will return an error if the innput data is inproperly quoted, or contains the wrong nubmer of fields.
    pub fn parse_row(&self, row: &[u8]) -> Result<Vec<String>, CsiAdapterError> {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(*self.cell_delimiter)
            .from_reader(row);
        if let Some(result) = rdr.records().next() {
            match result {
                Ok(rec) => Ok(rec.iter().map(|s| s.to_string()).collect()),
                Err(e) => Err(CsiAdapterError::CSV(CSVAdapterError::InvalidData(format!("CSV parse error: {e}")))),
            }
        } else {
            Err(CsiAdapterError::CSV(CSVAdapterError::InvalidData("Empty CSV row".to_string())))
        }
    }

    fn consume(&mut self, buf: &[u8]) -> Result<Option<CsiData>, CsiAdapterError> {
        self.buffer.extend_from_slice(buf);
        let rows = self.split_rows();
        if rows.is_empty() {
            return Ok(None);
        }
        let row = &rows[rows.len() - 1];
        let cells = self.parse_row(row)?;

        let csi_data = CSVAdapter::row_to_csi(cells)?;

        self.tmp_data = Some(csi_data);

        if let Some(data) = self.tmp_data.take() {
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    fn extract_complex(subcarrier_data: &str) -> Result<num_complex::Complex<f64>, CsiAdapterError> {
        let inner = subcarrier_data.trim_matches(|c| c == '(' || c == ')' || c == 'j' || c == '"');
        let mut split = inner.split('|');

        let real = split.next().ok_or(CsiAdapterError::InvalidInput)?;
        let imag = split.next().ok_or(CsiAdapterError::InvalidInput)?;

        // Ensure there are no extra parts
        if split.next().is_some() {
            return Err(CsiAdapterError::InvalidInput);
        }

        let real = real.parse::<f64>().map_err(|err| CsiAdapterError::FloatConversionError {
            err,
            input: real.to_string(),
        })?;
        let imag = imag.parse::<f64>().map_err(|err| CsiAdapterError::FloatConversionError {
            err,
            input: imag.to_string(),
        })?;
        Ok(Complex { re: real, im: imag })
    }

    /// Parses a CSV row (split into a vector) into CSI data.
    ///
    /// # Errors
    ///
    /// This function will return an error if the data is malformed.
    pub fn row_to_csi(cells: Vec<String>) -> Result<CsiData, CsiAdapterError> {
        if cells.len() != ROW_SIZE {
            return Err(CsiAdapterError::CSV(CSVAdapterError::InvalidData(format!(
                "Invalid number of columns in CSV row: {}",
                cells.len()
            ))));
        }
        let mut csi_data = CsiData {
            timestamp: cells[0]
                .trim_matches(|c| c == '\n' || c == '\0')
                .parse::<f64>()
                .map_err(|err| CsiAdapterError::FloatConversionError {
                    err,
                    input: cells[0].to_string(),
                })?,
            sequence_number: cells[1].parse::<u16>()?,
            ..Default::default()
        };
        let num_cores = cells[2].parse::<u8>()?;
        let num_streams = cells[3].parse::<u8>()?;
        let num_subcarriers = cells[4].parse::<u8>()?;
        csi_data.rssi = if cells[5].starts_with('(') {
            cells[5]
                .trim_matches(|c| c == '"' || c == '(' || c == ')')
                .split(',')
                .map(|rssi| rssi.parse::<u16>())
                .collect::<Result<Vec<_>, _>>()?
        } else {
            cells[5].split(',').map(|rssi| rssi.parse::<u16>()).collect::<Result<Vec<_>, _>>()?
        };
        let complex_values = cells[6]
            .trim_matches('"')
            .split(',')
            .map(CSVAdapter::extract_complex)
            .collect::<Result<Vec<Complex>, CsiAdapterError>>()?;
        let mut csi = vec![vec![vec![Complex::default(); num_subcarriers as usize]; num_streams as usize]; num_cores as usize];
        csi.iter_mut()
            .flat_map(|core| core.iter_mut())
            .flat_map(|stream| stream.iter_mut())
            .zip(complex_values.iter())
            .for_each(|(subcarrier, value)| *subcarrier = *value);
        csi_data.csi = csi;
        Ok(csi_data)
    }
}

impl std::default::Default for CSVAdapter<'_> {
    fn default() -> Self {
        CSVAdapter::new(Vec::new(), None, None, None)
    }
}

#[async_trait::async_trait]
impl CsiDataAdapter for CSVAdapter<'_> {
    /// Consumes bytes from the provided buffer and processes them into CSI data if a complete row is found.
    ///
    /// This function appends the incoming bytes to an internal buffer. If the buffer contains a complete
    /// CSV row (determined by the presence of a newline character not preceded by a backslash), it parses
    /// the row into a `CsiData` structure. The parsed data is temporarily stored and can be retrieved
    /// using the `reap` method. If the data is incomplete or invalid, the function will either wait for
    /// more data or return an error.
    ///
    /// # Arguments
    ///
    /// * `buf` - A slice of bytes representing the incoming data to be consumed.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the data is successfully consumed and processed or if the data is incomplete.
    /// * `Err(CsiAdapterError)` if an error occurs during parsing or processing.
    ///
    /// # Errors
    ///
    /// This function returns an error if the CSV row contains invalid data or if parsing fails.
    ///
    async fn produce(&mut self, msg: DataMsg) -> Result<Option<DataMsg>, CsiAdapterError> {
        // Check if the message is a raw frame
        match msg {
            DataMsg::RawFrame { bytes, .. } => match self.consume(&bytes)? {
                None => Ok(None),
                Some(csi) => Ok(Some(DataMsg::CsiFrame { csi })),
            },
            DataMsg::CsiFrame { csi } => Ok(Some(DataMsg::CsiFrame { csi })),
        }
    }
}

#[async_trait::async_trait]
impl ToConfig<DataAdapterConfig> for CSVAdapter<'_> {
    /// Converts the current `CSVAdapter` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `CSVAdapter`, returning a
    /// `DataAdapterConfig::CSV` variant. This enables the adapter's configuration
    /// to be serialized, exported, or stored as part of a broader system configuration.
    ///
    /// Since `CSVAdapter` does not currently hold any configurable fields, the
    /// resulting configuration is an empty struct.
    ///
    /// # Returns
    /// - `Ok(DataAdapterConfig::CSV)` on successful conversion.
    /// - `Err(TaskError)` if an error occurs (not applicable in this implementation).
    async fn to_config(&self) -> Result<DataAdapterConfig, TaskError> {
        Ok(DataAdapterConfig::CSV {})
    }
}

#[cfg(test)]
mod tests {
    use log::error;

    use super::*;
    use crate::network::rpc_message::SourceType;
    use crate::test_utils;

    #[test]
    fn test_extract_complex_valid() {
        // Typical format: "(real|imag)"
        let input = "(1.23|4.56j)";
        let result = CSVAdapter::extract_complex(input).unwrap();
        assert_eq!(result.re, 1.23);
        assert_eq!(result.im, 4.56);

        // With extra whitespace and quotes
        let input = "\"(7.89|-0.12j)\"";
        let result = CSVAdapter::extract_complex(input).unwrap();
        assert_eq!(result.re, 7.89);
        assert_eq!(result.im, -0.12);
    }

    #[test]
    fn test_extract_complex_invalid() {
        // Missing imaginary part
        let input = "(3.14|)";
        let result = CSVAdapter::extract_complex(input);
        assert!(result.is_err());

        // Too many parts
        let input = "(1.0|2.0|3.0)";
        let result = CSVAdapter::extract_complex(input);
        assert!(result.is_err());

        // Non-numeric input
        let input = "(abc|def)";
        let result = CSVAdapter::extract_complex(input);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_consume_split_row_in_two_steps() {
        let mut adapter = CSVAdapter::default();
        // Split a valid CSV row into two parts
        let part1 = b"9457616.210305953,45040,1,1,1,97,(";
        let part2 = b"0.4907193796689|-0.68406335243899j)\n";

        // First, send the incomplete part
        let msg1 = DataMsg::RawFrame {
            ts: 0.0,
            bytes: part1.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let result1 = adapter.produce(msg1).await.unwrap();
        // Should not yield a complete frame yet
        assert!(result1.is_none());
    }

    #[test]
    fn test_split_rows_single_row() {
        let mut adapter = CSVAdapter::new(b"row1col1,row1col2\n".to_vec(), None, None, None);
        let rows = adapter.split_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], b"row1col1,row1col2");
        assert_eq!(adapter.buffer, b"");
    }

    #[test]
    fn test_split_rows_multiple_rows() {
        let mut adapter = CSVAdapter::new(b"row1col1,row1col2\nrow2col1,row2col2\nrow3col1,row3col2\n".to_vec(), None, None, None);
        let rows = adapter.split_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], b"row1col1,row1col2");
        assert_eq!(rows[1], b"row2col1,row2col2");
        assert_eq!(rows[2], b"row3col1,row3col2");
        assert_eq!(adapter.buffer, b"");
    }

    #[test]
    fn test_split_rows_partial_row_left_in_buffer() {
        let mut adapter = CSVAdapter::new(b"row1col1,row1col2\nrow2col1,row2col2".to_vec(), None, None, None);
        let rows = adapter.split_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], b"row1col1,row1col2");
        assert_eq!(adapter.buffer, b"row2col1,row2col2");
    }

    #[test]
    fn test_split_rows_empty_buffer() {
        let mut adapter = CSVAdapter::new(Vec::new(), None, None, None);
        let rows = adapter.split_rows();
        assert!(rows.is_empty());
        assert_eq!(adapter.buffer, b"");
    }

    #[test]
    fn test_split_rows_custom_delimiter() {
        let custom_line_delim = b';';
        let mut adapter = CSVAdapter::new(
            b"row1col1,row1col2;row2col1,row2col2;partialrow".to_vec(),
            None,
            None,
            Some(&custom_line_delim),
        );
        let rows = adapter.split_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], b"row1col1,row1col2");
        assert_eq!(rows[1], b"row2col1,row2col2");
        assert_eq!(adapter.buffer, b"partialrow");
    }

    #[tokio::test]
    async fn test_consume_valid_data() {
        let mut adapter = CSVAdapter::default();
        let csv_data = b"9457616.210305953,45040,1,1,1,97,(0.4907193796689|-0.684063352438993j)\n";
        let msg = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let data = match adapter.produce(msg).await.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => csi,
            _ => panic!("Expected CsiFrame"),
        };

        assert_eq!(data.timestamp, 9457616.210305953);
        assert_eq!(data.sequence_number, 45040);
        assert_eq!(data.rssi, vec![97]);
        assert_eq!(
            data.csi,
            vec![vec![vec![Complex {
                re: 0.4907193796689,
                im: -0.684063352438993
            }],]]
        );
    }

    #[tokio::test]
    async fn test_consume_incomplete_data() {
        let mut adapter = CSVAdapter::default();
        let csv_data = b"1627584000.0,1,2,3,\"10,20\",\"(1|+2j),(3|+4j),(5|+6j),(7|+8j)\"";
        let msg = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        // this should return an InvalidInput error
        let result = adapter.produce(msg).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_consume_invalid_data() {
        let mut adapter = CSVAdapter::default();
        let csv_data = b"invalid,data,here\n";
        let msg = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let result = adapter.produce(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_reap_without_consume() {
        let mut adapter = CSVAdapter::default();
        let msg = DataMsg::RawFrame {
            ts: 0.0,
            bytes: b"".to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let result = adapter.produce(msg).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_consume_multiple_rows() {
        let mut adapter = CSVAdapter::default();
        let csv_data = b"5139255.620319567,13657,2,1,2,\"48,27\",\"(-0.24795687792212684|-0.7262670239309299j),(0.8454303851106912|+0.7649475667253236j),(-0.8925048482423406|+0.35672177778974534j),(0.5601050369340623|-0.9757985075283211j)\"\n1627584001.0,51825,2,1,2,\"10,53\",\"(-0.9336763181483387|+0.9137239452950752j),(0.04222732682994734|+0.4741629187802445j),(-0.24923809791108553|-0.6532018904054162j),(-0.13563524299387808|+0.8352370739609778j)\"\n";
        let msg = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let data1 = match adapter.produce(msg).await.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => csi,
            _ => panic!("Expected CsiFrame"),
        };
        assert_eq!(data1.timestamp, 1627584001.0);
        assert_eq!(data1.sequence_number, 51825);
    }

    #[tokio::test]
    async fn test_multiple_rows_multiple_consumes() {
        let mut adapter = CSVAdapter::default();

        let csv_data_1 = b"5139255.620319567,13657,2,1,2,\"48,27\",\"(-0.24795687792212684|-0.7262670239309299j),(0.8454303851106912|+0.7649475667253236j),(-0.8925048482423406|+0.35672177778974534j),(0.5601050369340623|-0.9757985075283211j)\"\n";
        let msg1 = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data_1.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let data1 = match adapter.produce(msg1).await.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => csi,
            _ => panic!("Expected CsiFrame"),
        };
        assert_eq!(data1.timestamp, 5139255.620319567);
        assert_eq!(data1.sequence_number, 13657);

        let csv_data_2 = b"1627584001.0,51825,2,1,2,\"10,53\",\"(-0.9336763181483387|+0.9137239452950752j),(0.04222732682994734|+0.4741629187802445j),(-0.24923809791108553|-0.6532018904054162j),(-0.13563524299387808|+0.8352370739609778j)\"\n";
        let msg2 = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data_2.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let data2 = match adapter.produce(msg2).await.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => csi,
            _ => panic!("Expected CsiFrame"),
        };
        assert_eq!(data2.timestamp, 1627584001.0);
        assert_eq!(data2.sequence_number, 51825);

        let csv_data_3 = b"1627584001.0,51825,2,1,2,\"10,53\",\"(-0.9336763181483387|+0.9137239452950752j),(0.04222732682994734|+0.4741629187802445j),(-0.24923809791108553|-0.6532018904054162j),(-0.13563524299387808|+0.8352370739609778j)\"\n";
        let msg3 = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data_3.to_vec(),
            source_type: crate::network::rpc_message::SourceType::ESP32,
        };
        let data3 = match adapter.produce(msg3).await.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => csi,
            _ => panic!("Expected CsiFrame"),
        };
        assert_eq!(data3.timestamp, 1627584001.0);
        assert_eq!(data3.sequence_number, 51825);
    }

    #[tokio::test]
    async fn test_consume_real_large_csv_file() {
        use std::fs;
        let mut adapter = CSVAdapter::default();
        let csv_file_option = test_utils::generate_csv_data_file();
        if csv_file_option.is_none() {
            error!("Skipped test, could not generate a CSV file");
            return;
        }
        let csv_file = csv_file_option.unwrap();
        let csv_data = fs::read(csv_file.path()).unwrap();
        let msg = DataMsg::RawFrame {
            ts: 0.0,
            bytes: csv_data,
            source_type: SourceType::ESP32,
        };
        // The adapter should process the last row
        let data = adapter.produce(msg).await.unwrap().unwrap();
        if let DataMsg::CsiFrame { csi } = data {
            // Check that the timestamp and sequence number are as expected for the last row
            // (You can update these values to match the actual last row in your CSV file)
            assert!(csi.timestamp > 0.0);
            assert!(csi.sequence_number > 0);
            assert!(!csi.rssi.is_empty());
            assert!(!csi.csi.is_empty());
        } else {
            panic!("Expected CsiFrame");
        }
    }
}
