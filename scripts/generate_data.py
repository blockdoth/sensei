import csv
import random

# This script generates a CSV file with random CSI data.
# The rust structs:
# pub type Complex = Complex64;
# type Csi = Vec<Vec<Vec<Complex>>>;
# pub struct CsiData {
# 	pub timestamp       : f64,       // Timestamp (from receival of first packet fragment)
# 	pub sequence_number : u16,       // Extracted sequence number
# 	pub rssi            : Vec<u16>,  // Antenna (per core)
# 	pub csi             : Csi        // A num_cores x num_streams x num_subcarrier array
# }


def generate_csi_row(
    max_cores, max_streams, max_subcarriers, row_num, always_generate_max=False
):
    """
    Generates a single row of random CSI data.

    Args:
        max_cores (int): Maximum number of cores.
        max_streams (int): Maximum number of streams per core.
        max_subcarriers (int): Maximum number of subcarriers.
        always_generate_max (bool): If True, always generate the maximum number of cores, streams, and subcarriers.

    Returns:
        list: A list representing a single row of CSI data.
    """
    timestamp = random.uniform(row_num, row_num + 1)  # Random timestamp
    sequence_number = random.randint(0, 65535)  # Random sequence number
    num_cores = (
        random.randint(1, max_cores) if not always_generate_max else max_cores
    )  # Random number of cores
    num_stream = (
        random.randint(1, max_streams) if not always_generate_max else max_streams
    )  # Random number of antennas per core
    num_subcarriers = (
        random.randint(1, max_subcarriers)
        if not always_generate_max
        else max_subcarriers
    )  # Random number of subcarriers

    # Generate random RSSI values for each core-antenna
    rssi = []
    for _ in range(num_cores):
        rssi.extend([random.randint(0, 100) for _ in range(num_stream)])

    # Generate random CSI values for each core-antenna-stream-subcarrier combination
    csi = []
    for _ in range(num_cores):
        for _ in range(num_stream):
            for _ in range(1):  # Assuming 1 stream per antenna
                csi.extend(
                    [
                        complex(random.uniform(-1, 1), random.uniform(-1, 1))
                        for _ in range(num_subcarriers)
                    ]
                )

    return [
        timestamp,
        sequence_number,
        num_cores,
        num_stream,
        num_subcarriers,
        ",".join(map(str, rssi)),
        ",".join(map(str, csi)),
    ]


def generate_csi_data_csv(
    file_path,
    num_rows=10000,
    max_cores=2,
    max_streams=4,
    max_subcarriers=64,
    always_generate_max=False,
):
    """
    Generates a CSV file with random CSI data, including cores.

    Args:
        file_path (str): Path to the output CSV file.
        num_rows (int): Number of rows to generate.
        max_cores (int): Maximum number of cores.
        max_streams (int): Maximum number of streams per core.
        max_subcarriers (int): Maximum number of subcarriers.
        always_generate_max (bool): If True, always generate the maximum number of cores, streams, and subcarriers.
    """
    # headers for the CSV file
    headers = [
        "timestamp",  # Timestamp (f64)
        "sequence_number",  # Sequence number (u16)
        "num_cores",  # Number of cores
        "num_streams",  # Number of streams per core
        "num_subcarriers",  # Number of subcarriers
        "rssi",  # RSSI values (comma-separated for each core-antenna)
        "csi",  # CSI values (comma-separated for each core-antenna-stream-subcarrier combination)
    ]

    with open(file_path, "w", newline="") as csvfile:
        writer = csv.writer(csvfile, lineterminator="\n")
        writer.writerow(headers)  # Write the headers

        for i in range(num_rows):
            row = generate_csi_row(
                max_cores, max_streams, max_subcarriers, i, always_generate_max
            )
            writer.writerow(row)
    # headers for the CSV file
    headers = [
        "timestamp",  # Timestamp (f64)
        "sequence_number",  # Sequence number (u16)
        "num_cores",  # Number of cores
        "num_streams",  # Number of streams per core
        "num_subcarriers",  # Number of subcarriers
        "rssi",  # RSSI values (comma-separated for each core-antenna)
        "csi",  # CSI values (comma-separated for each core-antenna-stream-subcarrier combination)
    ]


generate_csi_data_csv(
    "csi_data.csv",
    num_rows=10000,
    max_cores=2,
    max_streams=2,
    max_subcarriers=2,
    always_generate_max=True,
)
