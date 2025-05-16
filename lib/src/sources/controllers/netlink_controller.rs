use crate::errors::ControllerError;
use crate::controllers::Controller;
use std::fs::{self, OpenOptions};
use std::io::Write;
use tokio::process::Command;

#[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(default)]
pub struct NetworkCardControlParams {
    pub interface: String,
    pub center_freq_mhz: u32,
    pub bandwidth_mhz: u32,
    pub control_freq_mhz: Option<u32>,
    pub antenna_mask: Option<u8>,
}

impl Default for NetworkCardControlParams {
    fn default() -> Self {
        Self {
            interface: "wlp1s0".into(),
            center_freq_mhz: 2412,
            bandwidth_mhz: 20,
            control_freq_mhz: None,
            antenna_mask: None,
        }
    }
}

#[typetag::serde(name = "NetLink")]
#[async_trait::async_trait]
impl Controller for NetworkCardControlParams {
    async fn configure(&self) -> Result<(), ControllerError> {
        let mut freq_args = vec![
            self.center_freq_mhz.to_string(),
            self.bandwidth_mhz.to_string(),
        ];

        if self.bandwidth_mhz != 20 {
            if let Some(control_freq) = self.control_freq_mhz {
                freq_args.insert(0, control_freq.to_string());
            } else {
                return Err(ControllerError::MissingParameter(
                    "Bandwidths greater than 20 MHz require `control_freq_mhz`.".into(),
                ));
            }
        }

        Command::new("sudo")
            .arg("ip").arg("link").arg("set").arg("dev")
            .arg(&self.interface).arg("down")
            .spawn()?.wait().await?;

        Command::new("sudo")
            .arg("ifconfig").arg(&self.interface).arg("down")
            .spawn()?.wait().await?;

        Command::new("sudo")
            .arg("iwconfig").arg(&self.interface).arg("mode").arg("monitor")
            .spawn()?.wait().await?;

        Command::new("sudo")
            .arg("ifconfig").arg(&self.interface).arg("up")
            .spawn()?.wait().await?;

        Command::new("sudo")
            .arg("iw").arg("reg").arg("set").arg("US")
            .spawn()?.wait().await?;

        Command::new("sudo")
            .arg("iw").arg(&self.interface).arg("set").arg("freq")
            .args(&freq_args)
            .spawn()?.wait().await?;

        if let Some(rx_chainmask) = self.antenna_mask {
            let phy_name = get_phy_name(&self.interface)?;
            set_rx_chainmask(&phy_name, rx_chainmask)?;
        }

        Ok(())
    }
}

/// Get the phy name (e.g., "phy0") for a wireless interface
fn get_phy_name(interface: &str) -> Result<String, ControllerError> {
    // Path to the phy80211 symlink
    let phy_symlink = format!("/sys/class/net/{}/phy80211", interface);

    // Resolve the symlink to find the corresponding phyX
    let phy_path = fs::read_link(&phy_symlink)?;

    // Extract the last component of the path (e.g., "phy0")
    phy_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .ok_or(ControllerError::PhyName)
}

/// Set receive antenna chainmask
fn set_rx_chainmask(phy_name: &str, chainmask: u8) -> Result<(), ControllerError> {
    let path = format!(
        "/sys/kernel/debug/ieee80211/{}/iwlwifi/iwlmvm/rx_chainmask",
        phy_name
    );
    let mut file = OpenOptions::new().write(true).open(path)?;
    writeln!(file, "{}", chainmask)?;
    Ok(())
}
