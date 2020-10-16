//! Reactor used to initialize a node.

use cosy_macro::reactor;

use crate::{
    components::{small_network::NodeId, storage::Storage},
    protocol::Message,
    reactor::validator,
};

reactor!(Initializer {
  type Config = validator::Config;

  components: {
    chainspec = ChainspecLoader();
    storage = Storage();
    contract_runtime = ContractRuntime();
  }

  events: {
    storage = Event<Storage>;
  }

  requests: {
    StorageRequest<Storage> -> storage;
    ContractRuntimeRequest -> contract_runtime;

    // No network traffic during initialization, just discard.
    // TODO: Allow for "hard" discard, resulting in a crash?
    NetworkRequest<NodeId, Message> -> !;
  }

  announcements: {
    // We neither send nor process announcements.
  }
});

// TODO: Metrics
// TODO: is_stopped
// TODO: initialization/config processing

impl Initializer {
    /// Returns whether the initialization process completed successfully or not.
    pub fn stopped_successfully(&self) -> bool {
        self.chainspec.stopped_successfully()
    }
}