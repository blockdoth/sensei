/**
 * Module for controllers
 * Mofidied based on: wisense/sensei/lib/src/adapters/mod.rs
 * Originally authored by: Fabian Portner
 */

//! Source Controller
//! -----------------
//!
//! Controllers are used to configure the collection of data at a specific source.
//! This includes things like setting interfaces to monitor mode, configuration of
//! channel and bandwidth, antenna stream selection, etc.
//!
//! TODO: Should probably change this to also be a trait and implement structs for
//! it. Otherwise, we do not have any state, and thus no control to avoid repeating
//! the same operations frequently again.
#[cfg(feature = "pcap_source")]
pub mod nexmon;

#[cfg(feature = "netlink_source")]
pub mod nic;

#[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ControllerParams {
    #[cfg(feature = "pcap_source")]
    Nexmon(nexmon::NexmonControlParams),
    #[cfg(feature = "netlink_source")]
    NetworkCard(nic::NetworkCardControlParams),
}
