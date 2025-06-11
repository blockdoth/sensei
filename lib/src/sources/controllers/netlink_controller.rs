use std::fs::{self, OpenOptions};
use std::io::Write;

use tokio::process::Command;

use crate::ToConfig;
use crate::errors::{ControllerError, TaskError};
use crate::sources::DataSourceT;
use crate::sources::controllers::{Controller, ControllerParams};

/// Parameters for the Netlink controller, typically parsed from yaml file
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, schemars::JsonSchema)]
#[serde(default)]
pub struct NetlinkControllerParams {
    /// Name of the wireless interface to configure (e.g., "wlp1s0").
    pub interface: String,
    /// Center frequency in MHz (e.g., 2412 for channel 1 on 2.4GHz)
    pub center_freq_mhz: u32,
    /// Bandwidth in MHz. Must be 20 unless `control_freq_mhz` is also specified.
    pub bandwidth_mhz: u32,
    /// Optional control frequency in MHz for HT40/80/160 MHz configurations.
    pub control_freq_mhz: Option<u32>,
    /// Optional antenna mask to set the RX chain mask (e.g., 7 for 3 chains).
    pub antenna_mask: Option<u8>,
}

impl Default for NetlinkControllerParams {
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

/// Implementation of the `Controller` trait for NetlinkControllerParams.
///
/// This implementation issues system commands to configure the wireless
/// interface into monitor mode with the desired frequency and settings.
#[async_trait::async_trait]
impl Controller for NetlinkControllerParams {
    async fn apply(&self, _source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        let mut freq_args = vec![self.center_freq_mhz.to_string(), self.bandwidth_mhz.to_string()];

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
            .arg("ip")
            .arg("link")
            .arg("set")
            .arg("dev")
            .arg(&self.interface)
            .arg("down")
            .spawn()?
            .wait()
            .await?;

        Command::new("sudo")
            .arg("ifconfig")
            .arg(&self.interface)
            .arg("down")
            .spawn()?
            .wait()
            .await?;

        Command::new("sudo")
            .arg("iwconfig")
            .arg(&self.interface)
            .arg("mode")
            .arg("monitor")
            .spawn()?
            .wait()
            .await?;

        Command::new("sudo")
            .arg("ifconfig")
            .arg(&self.interface)
            .arg("up")
            .spawn()?
            .wait()
            .await?;

        Command::new("sudo").arg("iw").arg("reg").arg("set").arg("US").spawn()?.wait().await?;

        Command::new("sudo")
            .arg("iw")
            .arg(&self.interface)
            .arg("set")
            .arg("freq")
            .args(&freq_args)
            .spawn()?
            .wait()
            .await?;

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
    let phy_symlink = format!("/sys/class/net/{interface}/phy80211");

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
    let path = format!("/sys/kernel/debug/ieee80211/{phy_name}/iwlwifi/iwlmvm/rx_chainmask");
    let mut file = OpenOptions::new().write(true).open(path)?;
    writeln!(file, "{chainmask}")?;
    Ok(())
}

#[async_trait::async_trait]
impl ToConfig<ControllerParams> for NetlinkControllerParams {
    /// Converts the current `NetlinkControllerParams` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `NetlinkControllerParams`, allowing a live
    /// controller instance to be converted into a `ControllerParams::Netlink` variant.
    /// This is useful for persisting the controller's configuration to a file (e.g., YAML or JSON)
    /// or for reproducing its state programmatically.
    ///
    /// # Returns
    /// - `Ok(ControllerParams::Netlink)` containing a cloned version of the controller's parameters.
    /// - `Err(TaskError)` if an error occurs during conversion (not applicable in this implementation).
    async fn to_config(&self) -> Result<ControllerParams, TaskError> {
        Ok(ControllerParams::Netlink(self.clone()))
    }
}
