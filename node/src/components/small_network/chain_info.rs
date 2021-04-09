//! Network-related chain identification information.

// TODO: This module and `ChainId` should disappear in its entirety and the actual chainspec be made
// available.

use std::{collections::HashSet, net::SocketAddr};

use casper_types::ProtocolVersion;
use datasize::DataSize;

use super::Message;
use crate::{crypto::hash::Digest, types::Chainspec};

/// Data retained from the chainspec by the small networking component.
///
/// Typically this information is used for creating handshakes.
#[derive(DataSize, Debug)]
pub(crate) struct ChainInfo {
    /// Name of the network we participate in. We only remain connected to peers with the same
    /// network name as us.
    pub(super) network_name: String,
    /// The maximum message size for a network message, as supplied from the chainspec.
    pub(super) maximum_net_message_size: u32,
    /// The protocol version.
    pub(super) protocol_version: ProtocolVersion,
    /// Hash of the chainspec we are running with.
    pub(super) our_chainspec: Digest,
    /// The list of ancestors we support.
    pub(super) supported_ancestors: HashSet<Digest>,
}

impl ChainInfo {
    /// Create an instance of `ChainInfo` for testing.
    #[cfg(test)]
    pub fn create_for_testing() -> Self {
        ChainInfo {
            network_name: "rust-tests-network".to_string(),
            maximum_net_message_size: 22 * 1024 * 1024, // Hardcoded at 22M.
            protocol_version: ProtocolVersion::V1_0_0,

            // The test configuration does not deal with previous versions. Nodes will still match
            // up, as they share a version.
            our_chainspec: Digest::default(),
            supported_ancestors: Default::default(),
        }
    }

    /// Create a handshake based on chain identification data.
    pub(super) fn create_handshake<P>(&self, public_address: SocketAddr) -> Message<P> {
        Message::Handshake {
            network_name: self.network_name.clone(),
            public_address,
            protocol_version: self.protocol_version,
            chainspec: Some(self.our_chainspec),
            supports: self.supported_ancestors.clone(),
        }
    }

    /// Determines whether or not a given set of remote chainspec data is compatible with ours.
    pub(super) fn is_compatible_with(
        &self,
        their_chainspec: &Option<Digest>,
        their_supports: &HashSet<Digest>,
    ) -> bool {
        match their_chainspec {
            Some(their_chainspec) => {
                // If our chainspecs match 1:1, we are definitely compatible.
                if their_chainspec == &self.our_chainspec {
                    return true;
                }

                // Otherwise, ensure at least compatibility on one side.
                self.supported_ancestors.contains(their_chainspec)
                    || their_supports.contains(&self.our_chainspec)
            }
            None => {
                // Remote did not send a chainspec at all. We completely ignore chainspec
                // checking at this point, as they are likely on a older version.
                true
            }
        }
    }
}

impl From<&Chainspec> for ChainInfo {
    fn from(chainspec: &Chainspec) -> Self {
        ChainInfo {
            network_name: chainspec.network_config.name.clone(),
            maximum_net_message_size: chainspec.network_config.maximum_net_message_size,
            protocol_version: chainspec.protocol_version(),
            our_chainspec: chainspec.hash(),
            supported_ancestors: chainspec
                .protocol_config
                .supported_ancestors
                .iter()
                .cloned()
                .collect(),
        }
    }
}
