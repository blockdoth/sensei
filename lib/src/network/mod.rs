//! Network Module
//!
//! This module provides the core networking functionality for the Sensei system, including message types and TCP communication primitives.
//!
//! # Modules
//!
//! - [`rpc_message`]: Defines the types and structures for remote procedure call (RPC) messages exchanged between nodes and orchestrators, including message kinds, control commands, device/host status, and serialization helpers.
//! - [`tcp`]: Implements asynchronous TCP networking primitives, including message framing, sending/receiving, and connection handling traits for both clients and servers.
//!
//! The network module is designed to be used by both orchestrator and node components, providing a unified interface for network communication and message serialization.
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use lib::network::tcp::client::TcpClient;
//! use lib::network::rpc_message::RpcMessage;
//! // ...
//! ```

pub mod experiment_config;
pub mod rpc_message;
pub mod tcp;
