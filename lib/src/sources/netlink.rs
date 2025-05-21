use crate::errors::DataSourceError;
use crate::sources::DataSourceT;
use crate::sources::controllers::Controller;

use log::trace;
use netlink_sys::{Socket, SocketAddr, protocols::NETLINK_CONNECTOR};
use serde::Deserialize;

// Configuration structure for a Netlink source.
///
/// This struct is deserializable from YAML config files 
#[derive(Debug, Deserialize, Clone)]
pub struct NetlinkConfig {
    /// Netlink connector group ID to subscribe to.
    pub group: u32,
}


/// Netlink-based implementation of the [`DataSourceT`] trait.
///
/// Internally, this type maintains a raw socket and a reusable buffer for
/// reading messages. It handles netlink and connector protocol header parsing
/// to extract the payload intended for sinks.
pub struct NetlinkSource {
    config: NetlinkConfig,
    socket: Option<Socket>,
    buffer: [u8; 8192],
}

impl NetlinkSource {
    /// Create a new [`NetlinkSource`] from a configuration struct.
    ///
    /// # Errors
    /// Returns [`DataSourceError`] if configuration is invalid.
    pub fn new(config: NetlinkConfig) -> Result<Self, DataSourceError> {
        trace!("Creating new netlink source (group id: {})", config.group);
        Ok(Self {
            config,
            socket: None,
            buffer: [0; 8192],
        })
    }
}

/// Some netlink parsing helpers
const NLMSG_HDRLEN: usize = 16; // Size of the Netlink header in bytes
const NLCNMSG_HDRLEN: usize = 20; // Size of the connector message header in bytes
const NLMSG_DONE: u16 = 0x3; // End of Netlink message sequence

/// Parsed representation of a netlink message header.
/// Netlink header: https://docs.huihoo.com/doxygen/linux/kernel/3.7/structnlmsghdr.html
#[derive(Debug)]
struct NetlinkHeader {
    length: u32,
    message_type: u16,
    flags: u16,
    sequence_number: u32,
    port_id: u32,
}

impl NetlinkHeader {
    /// Parse a Netlink header from raw bytes
    fn parse(buf: &[u8]) -> Result<Self, DataSourceError> {
        if buf.len() < NLMSG_HDRLEN {
            return Err(DataSourceError::IncompletePacket);
        }

        Ok(Self {
            length: u32::from_ne_bytes(buf[0..4].try_into()?),
            message_type: u16::from_ne_bytes(buf[4..6].try_into()?),
            flags: u16::from_ne_bytes(buf[6..8].try_into()?),
            sequence_number: u32::from_ne_bytes(buf[8..12].try_into()?),
            port_id: u32::from_ne_bytes(buf[12..16].try_into()?),
        })
    }
}

/// Parsed representation of a connector protocol message header.
/// This is the message sent by the connector protocol:
/// https://www.kernel.org/doc/Documentation/connector/connector.txt
#[derive(Debug)]
struct ConnectorMessageHeader {
    idx: u32,
    val: u32,
    seq: u32,
    ack: u32,
    len: u32,
}

impl ConnectorMessageHeader {
    /// Parse connector message header
    fn parse(buf: &[u8]) -> Result<Self, DataSourceError> {
        if buf.len() < NLMSG_HDRLEN {
            return Err(DataSourceError::IncompletePacket);
        }

        Ok(Self {
            idx: u32::from_ne_bytes(buf[0..4].try_into()?),
            val: u32::from_ne_bytes(buf[4..8].try_into()?),
            seq: u32::from_ne_bytes(buf[8..12].try_into()?),
            ack: u32::from_ne_bytes(buf[12..16].try_into()?),
            len: u32::from_ne_bytes(buf[16..20].try_into()?),
        })
    }
}

/// Extract connector message from netlink message.
fn get_connector_payload(buf: &[u8]) -> Result<&[u8], DataSourceError> {
    let mut offset = 0;

    let header = NetlinkHeader::parse(buf)?;
    offset += NLMSG_HDRLEN;

    if header.message_type != NLMSG_DONE {
        log::error!("Unhandled message type found!");
        return Err(DataSourceError::NotImplemented(format!(
            "Unhandled message type {}",
            header.message_type
        )));
    }

    let cn_header = ConnectorMessageHeader::parse(&buf[offset..])?;
    offset += NLCNMSG_HDRLEN;

    let payload = &buf[offset..offset + cn_header.len as usize];
    Ok(payload)
}


/// Implements the CSI data source trait for netlink communication.
///
/// Handles startup, shutdown, and frame-by-frame payload reading from the
/// Linux netlink connector interface.
#[async_trait::async_trait]
impl DataSourceT for NetlinkSource {
    async fn start(&mut self) -> Result<(), DataSourceError> {
        trace!(
            "Connecting to netlink socket with group id: {}",
            self.config.group
        );
        if self.socket.is_some() {
            return Ok(());
        }

        let mut sock = Socket::new(NETLINK_CONNECTOR)?;
        let pid = std::process::id();
        let proc_addr = SocketAddr::new(pid, self.config.group);

        // Make non-blocking.
        trace!("Configuring netlink socket (binding and adding group subscription)");

        // Bind the socket to the process address and join the group
        sock.bind(&proc_addr).map_err(|err| {
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                DataSourceError::PermissionDenied
            } else {
                DataSourceError::Io(err)
            }
        })?;

        sock.add_membership(self.config.group)?;
        sock.set_non_blocking(true)?;

        trace!("Socket successfully configured!");
        self.socket = Some(sock);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!(
            "Stopping data collection from netlink group id: {}",
            self.config.group
        );
        if let Some(sock) = self.socket.as_mut() {
            sock.drop_membership(self.config.group)?;
        }
        self.socket = None;
        Ok(())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
        if self.socket.is_none() {
            return Err(DataSourceError::ReadBeforeStart);
        }

        let bytes_read = self.socket_read().await?;
        if bytes_read == 0 {
            return Ok(0);
        }

        let payload = get_connector_payload(&self.buffer[..bytes_read])?;
        buf[..payload.len()].copy_from_slice(payload);
        Ok(payload.len())
    }
}

impl NetlinkSource {
    /// Reads raw bytes from the netlink socket into the internal buffer.
    ///
    /// This function performs a non-blocking read and returns the number of bytes read.
    async fn socket_read(&mut self) -> Result<usize, DataSourceError> {
        let mut buf = &mut self.buffer[..];
        let before = buf.len();

        // Attempt to read from the socket
        let result = self
            .socket
            .as_mut()
            .expect("socket_read must only be called after existence of socket is checked")
            .recv_from(&mut buf, 0);

        match result {
            Ok(_addr) => {
                let after = buf.len();
                let bytes_read = before - after;
                Ok(bytes_read)
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // This means there is no data. We set the socket to nonblock, so this is
                // expected.
                Ok(0)
            }
            Err(e) => {
                // Propagate other errors
                Err(DataSourceError::Io(e))
            }
        }
    }
}
