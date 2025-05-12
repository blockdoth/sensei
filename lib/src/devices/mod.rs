use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct DeviceCfg {}

impl FromStr for DeviceCfg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(DeviceCfg {})
    }
}
