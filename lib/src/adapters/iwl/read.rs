use crate::adapters::CsiDataAdapter;
use crate::adapters::iwl::IwlAdapter;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::time::{self, Duration, Instant};

/// Reads and prints live CSI data from a device file for a specified number of minutes.
pub async fn print_live_csi_data(path: &str, duration_minutes: u64) {
    let mut adapter = IwlAdapter::new(true);

    let file = match File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to open CSI file '{path}': {e:?}");
            return;
        }
    };

    let mut reader = BufReader::new(file);
    let mut buffer = vec![0u8; 4096];
    let end_time = Instant::now() + Duration::from_secs(duration_minutes * 60);

    while Instant::now() < end_time {
        let n = match reader.read(&mut buffer).await {
            Ok(n) => n,
            Err(e) => {
                eprintln!("Failed to read CSI data: {e:?}");
                return;
            }
        };

        if n == 0 {
            time::sleep(Duration::from_millis(10)).await;
            continue;
        }

        let data = &buffer[..n];

        match adapter.consume(data).await {
            Ok(_) => {
                if let Ok(Some(csi_data)) = adapter.reap().await {
                    println!("{csi_data:#?}");
                }
            }
            Err(e) => {
                eprintln!("Error consuming CSI data: {e:?}");
                continue;
            }
        }
    }

    println!("Done collecting CSI data for {duration_minutes} minute(s).");
}

#[cfg(test)]
mod tests {
    use super::print_live_csi_data;

    #[tokio::test]
    async fn test_print_csi_from_file() {
        let path = "tmp/csi.dat";
        let minutes = 0; // Run for 1 minute
        print_live_csi_data(path, minutes).await;
    }
}
