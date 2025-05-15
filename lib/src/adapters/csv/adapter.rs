use crate::adapters::CsiDataAdapter;
use crate::csi_types::{Complex, CsiData};
use crate::errors::CsiAdapterError;
use std::fs::File;
use std::io::Write;
use std::io::{self, Read, Seek, SeekFrom};
use std::vec;

const DEFAULT_LINE_DELIM: u8 = b'\n';
const DEFAULT_CELL_DELIM: u8 = b',';
const ROW_SIZE: usize = 7;

pub struct CSVAdapter<'a> {
    buffer: Vec<u8>,
    cursor_pos: usize,
    tmp_data: Option<CsiData>,
    cell_delimiter: &'a u8,
    line_delimiter: &'a u8,
}

#[allow(clippy::needless_lifetimes)] // TODO: fix this
impl<'a> CSVAdapter<'a> {
    pub fn new(
        buffer: Vec<u8>,
        tmp_data: Option<CsiData>,
        cell_delimiter: &'a u8,
        line_delimiter: &'a u8,
    ) -> Self {
        Self {
            buffer,
            tmp_data,
            cursor_pos: 0,
            cell_delimiter,
            line_delimiter,
        }
    }
}

impl std::default::Default for CSVAdapter<'_> {
    fn default() -> Self {
        CSVAdapter::new(Vec::new(), None, &DEFAULT_CELL_DELIM, &DEFAULT_LINE_DELIM)
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
    /// # Examples
    ///
    /// ```
    /// let mut adapter = CSVAdapter::default();
    /// let csv_data = b"1627584000.0,1,2,3,10;20,1+2i|3+4i;5+6i|7+8i\n";
    /// adapter.consume(csv_data).await.unwrap();
    /// ```
    async fn produce(&mut self, buf: &[u8]) -> Result<Option<CsiData>, CsiAdapterError> {
        // Append the incoming bytes to the buffer
        self.buffer.extend_from_slice(buf);
        // Check if the buffer contains a complete row
        let mut found = false;
        // Find the first EOL character in the buffer
        let x: Vec<_> = self.buffer.split(|c| c == self.line_delimiter).collect();
        // we care about the second to last row, as that's the most recent complete row
        if x.len() < 2 {
            return Ok(None); // No complete row found, return Ok. Data might be incomplete.
        }
        let row_buffer = x[x.len() - 2];
        // EOL found, process the buffer
        let mut csi_data = CsiData::default();
        // split the buffer into cells
        // | timestamp | sequence_number |num_cores | num_antennas | num_subcarriers | rssi | csi |
        // |-----------|-----------------|----------|--------------|-----------------|------|-----|
        let mut in_quotes = false;
        let row = row_buffer
            .split(|&byte| {
                if byte == b'"' {
                    in_quotes = !in_quotes;
                }
                byte == *self.cell_delimiter && !in_quotes
            })
            .map(|cell| String::from_utf8(cell.to_vec()).unwrap())
            .collect::<Vec<_>>();
        // parse the row. This includes a bunch of formatting and parsing checks
        if row.len() != ROW_SIZE {
            return Err(CsiAdapterError::CSV(super::CSVAdapterError::InvalidData(
                format!("Invalid number of columns in CSV row: {}", row.len()),
            )));
        }
        csi_data.timestamp = row[0].trim_matches('\n').parse::<f64>().map_err(|_| {
            CsiAdapterError::CSV(super::CSVAdapterError::InvalidData(format!(
                "Invalid timestamp: {}",
                row[0]
            )))
        })?;
        csi_data.sequence_number = row[1].parse::<u16>().map_err(|_| {
            CsiAdapterError::CSV(super::CSVAdapterError::InvalidData(format!(
                "Invalid sequence number: {}",
                row[1]
            )))
        })?;
        let num_cores = row[2].parse::<u8>().map_err(|_| {
            CsiAdapterError::CSV(super::CSVAdapterError::InvalidData(format!(
                "Invalid number of cores: {}",
                row[2]
            )))
        })?;
        let num_streams = row[3].parse::<u8>().map_err(|_| {
            CsiAdapterError::CSV(super::CSVAdapterError::InvalidData(format!(
                "Invalid number of streams: {}",
                row[2]
            )))
        })?;
        let num_subcarriers = row[4].parse::<u8>().map_err(|_| {
            CsiAdapterError::CSV(super::CSVAdapterError::InvalidData(format!(
                "Invalid number of subcarriers: {}",
                row[3]
            )))
        })?;
        csi_data.rssi = if row[5].starts_with('(') {
            row[5]
                .get(1..row[5].len() - 2)
                .unwrap()
                .split(',')
                .map(|rssi| rssi.parse::<u16>().unwrap_or_default())
                .collect::<Vec<_>>()
        } else {
            row[5]
                .split(',')
                .map(|rssi| rssi.parse::<u16>().unwrap_or_default())
                .collect::<Vec<_>>()
        };
        let comples_values = row[6]
            .split(',') // Split by subcarriers
            .map(|subcarrier_data| {
                // Remove the parentheses and 'j' from the complex number
                let inner =
                    subcarrier_data.trim_matches(|c| c == '\"' || c == '(' || c == ')' || c == 'j');
                let parts: Vec<&str> = if let Some(index) = inner.rfind(['+', '-']) {
                    vec![&inner[..index], &inner[index..]]
                } else {
                    vec![inner]
                };
                assert!(parts.len() == 2, "Invalid complex number format: {inner}");
                let real = parts[0].parse::<f64>().unwrap();
                let imag = parts[1].parse::<f64>().unwrap();
                Complex { re: real, im: imag }
            })
            .collect::<Vec<Complex>>();

        // turn the complex values into a 3D array
        let mut csi =
            vec![
                vec![vec![Complex::default(); num_subcarriers as usize]; num_streams as usize];
                num_cores as usize
            ];
        csi.iter_mut()
            .flat_map(|core| core.iter_mut())
            .flat_map(|stream| stream.iter_mut())
            .zip(comples_values.iter())
            .for_each(|(subcarrier, &value)| *subcarrier = value);
        csi_data.csi = csi;

        assert!(
            csi_data.csi.len() == num_cores as usize,
            "Invalid number of cores in CSV row: {}, expected: {}",
            csi_data.csi.len(),
            num_cores
        );
        assert!(
            csi_data.csi[0].len() == num_streams as usize,
            "Invalid number of streams in CSV row: {}, expected: {}",
            csi_data.csi[0].len(),
            num_streams
        );
        assert!(
            csi_data.csi[0][0].len() == num_subcarriers as usize,
            "Invalid number of subcarriers in CSV row: {}, expected: {}",
            csi_data.csi[0][0].len(),
            num_subcarriers
        );
        // check if the row has the expected number of columns
        assert!(
            csi_data.rssi.len() == num_cores as usize,
            "Invalid number of cores in CSV row: {}, expected: {}",
            csi_data.rssi.len(),
            num_cores
        );

        self.tmp_data = Some(csi_data);

        if let Some(data) = self.tmp_data.take() {
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_consume_valid_data() {
        let mut adapter = CSVAdapter::default();
        let csv_data = b"9457616.210305953,45040,1,1,1,97,(0.4907193796689-0.684063352438993j)\n";
        let data = adapter.produce(csv_data).await.unwrap().unwrap();

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
        let csv_data = b"1627584000.0,1,2,3,10;20,1+2i|3+4i;5+6i|7+8i";
        let data = adapter.produce(csv_data).await.unwrap();
        assert!(data.is_none());
    }

    #[tokio::test]
    async fn test_consume_invalid_data() {
        let mut adapter = CSVAdapter::default();
        let csv_data = b"invalid,data,here\n";
        let result = adapter.produce(csv_data).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_reap_without_consume() {
        let mut adapter = CSVAdapter::default();
        let data = adapter.produce(b"").await.unwrap();
        assert!(data.is_none());
    }

    #[tokio::test]
    async fn test_consume_multiple_rows() {
        let mut adapter = CSVAdapter::default();
        let csv_data = b"5139255.620319567,13657,2,1,2,\"48,27\",\"(-0.24795687792212684-0.7262670239309299j),(0.8454303851106912+0.7649475667253236j),(-0.8925048482423406+0.35672177778974534j),(0.5601050369340623-0.9757985075283211j)\"\n1627584001.0,51825,2,1,2,\"10,53\",\"(-0.9336763181483387+0.9137239452950752j),(0.04222732682994734+0.4741629187802445j),(-0.24923809791108553-0.6532018904054162j),(-0.13563524299387808+0.8352370739609778j)\"\n";

        let data1 = adapter.produce(csv_data).await.unwrap().unwrap();
        assert_eq!(data1.timestamp, 5139255.620319567);
        assert_eq!(data1.sequence_number, 13657);
    }

    #[tokio::test]
    async fn test_multiple_rows_multiple_consumes() {
        let mut adapter = CSVAdapter::default();

        let csv_data_1 = b"5139255.620319567,13657,2,1,2,\"48,27\",\"(-0.24795687792212684-0.7262670239309299j),(0.8454303851106912+0.7649475667253236j),(-0.8925048482423406+0.35672177778974534j),(0.5601050369340623-0.9757985075283211j)\"\n";
        let data1 = adapter.produce(csv_data_1).await.unwrap().unwrap();
        assert_eq!(data1.timestamp, 5139255.620319567);
        assert_eq!(data1.sequence_number, 13657);

        let csv_data_2 = b"1627584001.0,51825,2,1,2,\"10,53\",\"(-0.9336763181483387+0.9137239452950752j),(0.04222732682994734+0.4741629187802445j),(-0.24923809791108553-0.6532018904054162j),(-0.13563524299387808+0.8352370739609778j)\"\n";
        let data2 = adapter.produce(csv_data_2).await.unwrap().unwrap();
        assert_eq!(data2.timestamp, 1627584001.0);
        assert_eq!(data2.sequence_number, 51825);

        let csv_data_3 = b"1627584001.0,51825,2,1,2,\"10,53\",\"(-0.9336763181483387+0.9137239452950752j),(0.04222732682994734+0.4741629187802445j),(-0.24923809791108553-0.6532018904054162j),(-0.13563524299387808+0.8352370739609778j)\"\n";
        let data3 = adapter.produce(csv_data_3).await.unwrap().unwrap();
        assert_eq!(data3.timestamp, 1627584001.0);
        assert_eq!(data3.sequence_number, 51825);
    }
}
