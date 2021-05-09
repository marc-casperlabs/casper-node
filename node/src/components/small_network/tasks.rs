//! Tasks run by the component.

use std::{
    error::Error as StdError,
    fmt::{self, Debug, Display, Formatter},
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::Context;

use futures::{
    future::{self, Either},
    stream::{SplitSink, SplitStream},
    Future, SinkExt, StreamExt,
};
use openssl::{
    error::ErrorStack,
    pkey::{PKey, Private},
    ssl::{self, Ssl},
};
use prometheus::IntGauge;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    net::TcpStream,
    sync::{mpsc::UnboundedReceiver, watch},
};
use tokio_openssl::SslStream;
use tracing::{
    debug, error_span,
    field::{self, Empty},
    info, warn, Instrument, Span,
};

use super::{
    chain_info::ChainInfo,
    counting_format::{ConnectionId, Role},
    error::{display_error, Error, Result},
    framed, Event, FramedTransport, Message, Payload, Transport,
};
use crate::{
    components::networking_metrics::NetworkingMetrics,
    reactor::{EventQueueHandle, QueueKind},
    tls::{self, TlsCert, ValidationError},
    types::NodeId,
};

// TODO: Constants that need to be made configurable.

/// Maximum time allowed to send or receive a handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Network handshake reader for single handshake message received by outgoing connection.
pub(super) async fn read_handshake<REv, P>(
    event_queue: EventQueueHandle<REv>,
    mut stream: SplitStream<FramedTransport<P>>,
    our_id: NodeId,
    peer_id: NodeId,
    peer_addr: SocketAddr,
) where
    P: DeserializeOwned + Send + Display + Payload,
    REv: From<Event<P>>,
{
    if let Some(Ok(msg @ Message::Handshake { .. })) = stream.next().await {
        debug!(%our_id, %msg, %peer_id, "handshake received");
        return event_queue
            .schedule(
                Event::IncomingMessage {
                    peer_id: Box::new(peer_id),
                    msg: Box::new(msg),
                },
                QueueKind::NetworkIncoming,
            )
            .await;
    }
    warn!(%our_id, %peer_id, "receiving handshake failed, closing connection");
    event_queue
        .schedule(
            Event::OutgoingDropped {
                peer_id: Box::new(peer_id),
                peer_addr: Box::new(peer_addr),
                error: Box::new(None),
            },
            QueueKind::Network,
        )
        .await
}

/// Initiates a TLS connection to a remote address.
pub(super) async fn connect_outgoing(
    peer_addr: SocketAddr,
    our_certificate: Arc<TlsCert>,
    secret_key: Arc<PKey<Private>>,
) -> Result<(NodeId, Transport)> {
    let ssl = tls::create_tls_connector(&our_certificate.as_x509(), &secret_key)
        .context("could not create TLS connector")?
        .configure()
        .and_then(|mut config| {
            config.set_verify_hostname(false);
            config.into_ssl("this-will-not-be-checked.example.com")
        })
        .map_err(Error::ConnectorConfiguration)?;

    let stream = TcpStream::connect(peer_addr)
        .await
        .context("TCP connection failed")?;

    let mut tls_stream = SslStream::new(ssl, stream).context("tls handshake failed")?;
    SslStream::connect(Pin::new(&mut tls_stream))
        .await
        .map_err(Error::SslConnectionFailed)?;

    let peer_cert = tls_stream
        .ssl()
        .peer_certificate()
        .ok_or(Error::NoServerCertificate)?;

    let peer_id = tls::validate_cert(peer_cert)?.public_key_fingerprint();

    Ok((NodeId::from(peer_id), tls_stream))
}

/// A context holding all relevant information for networking communication shared across tasks.
pub(crate) struct NetworkContext<REv>
where
    REv: 'static,
{
    pub(super) event_queue: EventQueueHandle<REv>,
    pub(super) our_id: NodeId,
    pub(super) our_cert: Arc<TlsCert>,
    pub(super) secret_key: Arc<PKey<Private>>,
    pub(super) net_metrics: Weak<NetworkingMetrics>,
    pub(super) chain_info: Arc<ChainInfo>,
    pub(super) public_addr: SocketAddr,
}

/// A connection-specific error.
#[derive(Debug, Error, Serialize)]
pub enum ConnectionError {
    /// Failed to create TLS acceptor.
    #[error("failed to create acceptor")]
    AcceptorCreation(
        #[serde(skip_serializing)]
        #[source]
        ErrorStack,
    ),
    /// Handshaking error.
    #[error("TLS handshake error")]
    TlsHandshake(
        #[serde(skip_serializing)]
        #[source]
        ssl::Error,
    ),
    /// Client failed to present certificate.
    #[error("no client certificate presented")]
    NoClientCertificate,
    /// TLS validation error.
    #[error("TLS validation error of peer certificate")]
    PeerCertificateInvalid(#[source] ValidationError),
    /// Failed to send handshake.
    #[error("handshake send failed")]
    HandshakeSend(
        #[serde(skip_serializing)]
        #[source]
        IoError<io::Error>,
    ),
    /// Failed to receive handshake.
    #[error("handshake receive failed")]
    HandshakeRecv(
        #[serde(skip_serializing)]
        #[source]
        IoError<io::Error>,
    ),
    /// Peer reported a network name that does not match ours.
    #[error("peer is on different network: {0}")]
    WrongNetwork(String),
    /// Peer sent a non-handshake message as its first message.
    #[error("peer did not send handshake")]
    DidNotSendHandshake,
}

/// Outcome of an incoming connection negotiation.
#[derive(Debug, Serialize)]
pub enum IncomingConnection<P> {
    /// The connection failed early on, before even a peer's [`NodeId`] could be determined.
    FailedEarly {
        /// Remote port the peer dialed us from.
        peer_addr: SocketAddr,
        /// Error causing the failure.
        error: ConnectionError,
    },
    /// Connection failed after TLS was successfully established; thus we have a valid [`NodeId`].
    Failed {
        /// Remote port the peer dialed us from.
        peer_addr: SocketAddr,
        /// Peer's [`NodeId`].
        peer_id: NodeId,
        /// Error causing the failure.
        error: ConnectionError,
    },
    /// Connection turned out to be a loopback connection.
    Loopback,
    /// Connection successfully established.
    Established {
        /// Remote port the peer dialed us from.
        peer_addr: SocketAddr,
        /// Public address advertised by the peer.
        public_addr: SocketAddr,
        /// Peer's [`NodeId`].
        peer_id: NodeId,
        /// Stream of incoming messages. for incoming connections.
        #[serde(skip_serializing)]
        stream: SplitStream<FramedTransport<P>>,
    },
}

impl<P> Display for IncomingConnection<P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            IncomingConnection::FailedEarly { peer_addr, error } => {
                write!(f, "early failure from {}: {}", peer_addr, error)
            }
            IncomingConnection::Failed {
                peer_addr,
                peer_id,
                error,
            } => write!(f, "failure from {}/{}: {}", peer_addr, peer_id, error),
            IncomingConnection::Loopback => f.write_str("loopback"),
            IncomingConnection::Established {
                peer_addr,
                public_addr,
                peer_id,
                stream: _,
            } => write!(
                f,
                "connection established from {}/{}; public: {}",
                peer_addr, peer_id, public_addr
            ),
        }
    }
}

/// Handles an incoming connection.
///
/// Sets up a TLS stream and performs the protocol handshake.
async fn handle_incoming<P, REv>(
    context: Arc<NetworkContext<REv>>,
    stream: TcpStream,
    peer_addr: SocketAddr,
) -> IncomingConnection<P>
where
    REv: From<Event<P>> + 'static,
    P: Payload,
    for<'de> P: Serialize + Deserialize<'de>,
    for<'de> Message<P>: Serialize + Deserialize<'de>,
{
    let (peer_id, transport) =
        match server_setup_tls(stream, &context.our_cert, &context.secret_key).await {
            Ok(value) => value,
            Err(error) => {
                return IncomingConnection::FailedEarly { peer_addr, error };
            }
        };

    // Register the `peer_id` on the [`Span`] for logging the ID from here on out.
    Span::current().record("peer_id", &field::display(peer_id));

    if peer_id == context.our_id {
        info!("incoming loopback connection");
        return IncomingConnection::Loopback;
    }

    debug!("TLS connection established");

    // Setup connection sink and stream.
    let mut transport = framed::<P>(
        context.net_metrics.clone(),
        ConnectionId::from_connection(transport.ssl(), context.our_id, peer_id),
        transport,
        Role::Listener,
        context.chain_info.maximum_net_message_size,
    );

    // Negotiate the handshake, concluding the incoming connection process.
    match negotiate_handshake(&context, &mut transport).await {
        Ok(public_addr) => {
            // Close the receiving end of the transport.
            let (_sink, stream) = transport.split();

            IncomingConnection::Established {
                peer_addr,
                public_addr,
                peer_id,
                stream,
            }
        }
        Err(error) => IncomingConnection::Failed {
            peer_addr,
            peer_id,
            error,
        },
    }
}

/// Server-side TLS setup.
///
/// This function groups the TLS setup into a convenient function, enabling the `?` operator.
pub(super) async fn server_setup_tls(
    stream: TcpStream,
    cert: &TlsCert,
    secret_key: &PKey<Private>,
) -> ::std::result::Result<(NodeId, Transport), ConnectionError> {
    let mut tls_stream = tls::create_tls_acceptor(&cert.as_x509().as_ref(), &secret_key.as_ref())
        .and_then(|ssl_acceptor| Ssl::new(ssl_acceptor.context()))
        .and_then(|ssl| SslStream::new(ssl, stream))
        .map_err(ConnectionError::AcceptorCreation)?;

    SslStream::accept(Pin::new(&mut tls_stream))
        .await
        .map_err(ConnectionError::TlsHandshake)?;

    // We can now verify the certificate.
    let peer_cert = tls_stream
        .ssl()
        .peer_certificate()
        .ok_or(ConnectionError::NoClientCertificate)?;

    Ok((
        NodeId::from(
            tls::validate_cert(peer_cert)
                .map_err(ConnectionError::PeerCertificateInvalid)?
                .public_key_fingerprint(),
        ),
        tls_stream,
    ))
}

/// IO operation that can time out.
#[derive(Debug, Error)]
pub enum IoError<E>
where
    E: StdError + 'static,
{
    /// IO operation timed out.
    #[error("io timeout")]
    Timeout,
    /// Non-timeout IO error.
    #[error(transparent)]
    Error(#[from] E),
    /// Unexpected close/end-of-file.
    #[error("closed unexpectedly")]
    UnexpectedEof,
}

/// Performs an IO-operation that can time out.
async fn io_timeout<F, T, E>(duration: Duration, future: F) -> ::std::result::Result<T, IoError<E>>
where
    F: Future<Output = ::std::result::Result<T, E>>,
    E: StdError + 'static,
{
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_elapsed| IoError::Timeout)?
        .map_err(IoError::Error)
}

/// Performs an IO-operation that can time out or result in a closed connection.
async fn io_opt_timeout<F, T, E>(
    duration: Duration,
    future: F,
) -> ::std::result::Result<T, IoError<E>>
where
    F: Future<Output = Option<::std::result::Result<T, E>>>,
    E: StdError + 'static,
{
    let item = tokio::time::timeout(duration, future)
        .await
        .map_err(|_elapsed| IoError::Timeout)?;

    match item {
        Some(Ok(value)) => Ok(value),
        Some(Err(err)) => Err(IoError::Error(err)),
        None => Err(IoError::UnexpectedEof),
    }
}

async fn negotiate_handshake<P, REv>(
    context: &NetworkContext<REv>,
    transport: &mut FramedTransport<P>,
) -> std::result::Result<SocketAddr, ConnectionError>
where
    P: Payload,
{
    // Send down a handshake and expect one in response.
    let handshake = context.chain_info.create_handshake(context.public_addr);

    io_timeout(HANDSHAKE_TIMEOUT, transport.send(handshake))
        .await
        .map_err(ConnectionError::HandshakeSend)?;

    let remote_handshake = io_opt_timeout(HANDSHAKE_TIMEOUT, transport.next())
        .await
        .map_err(ConnectionError::HandshakeRecv)?;

    if let Message::Handshake {
        network_name,
        public_addr,
        protocol_version,
    } = remote_handshake
    {
        debug!(%protocol_version, "handshake received");

        // The handshake was valid, we can check the network name.
        if network_name != context.chain_info.network_name {
            Err(ConnectionError::WrongNetwork(network_name))
        } else {
            Ok(public_addr)
        }
    } else {
        // Received a non-handshake, this is an error.
        Err(ConnectionError::DidNotSendHandshake)
    }
}

/// Core accept loop for the networking server.
pub(super) async fn server<P, REv>(
    context: Arc<NetworkContext<REv>>,
    listener: tokio::net::TcpListener,
    mut shutdown_receiver: watch::Receiver<()>,
) where
    REv: From<Event<P>> + Send,
    P: Payload,
{
    // The server task is a bit tricky, since it has to wait on incoming connections while at the
    // same time shut down if the networking component is dropped, otherwise the TCP socket will
    // stay open, preventing reuse.

    // We first create a future that never terminates, handling incoming connections:
    let accept_connections = async {
        loop {
            // We handle accept errors here, since they can be caused by a temporary resource
            // shortage or the remote side closing the connection while it is waiting in
            // the queue.
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    // The span setup here is used throughout the entire lifetime of the connection.
                    let span = error_span!("incoming", %peer_addr, peer_id=Empty);

                    let context = context.clone();
                    let handler_span = span.clone();
                    tokio::spawn(
                        async move {
                            let incoming =
                                handle_incoming(context.clone(), stream, peer_addr).await;
                            context
                                .event_queue
                                .schedule(
                                    Event::IncomingConnection {
                                        incoming: Box::new(incoming),
                                        span,
                                    },
                                    QueueKind::NetworkIncoming,
                                )
                                .await;
                        }
                        .instrument(handler_span),
                    );
                }

                // TODO: Handle resource errors gracefully.
                //       In general, two kinds of errors occur here: Local resource exhaustion,
                //       which should be handled by waiting a few milliseconds, or remote connection
                //       errors, which can be dropped immediately.
                //
                //       The code in its current state will consume 100% CPU if local resource
                //       exhaustion happens, as no distinction is made and no delay introduced.
                Err(ref err) => {
                    warn!(%context.our_id, err=display_error(err), "dropping incoming connection during accept")
                }
            }
        }
    };

    let shutdown_messages = async move { while shutdown_receiver.changed().await.is_ok() {} };

    // Now we can wait for either the `shutdown` channel's remote end to do be dropped or the
    // infinite loop to terminate, which never happens.
    match future::select(Box::pin(shutdown_messages), Box::pin(accept_connections)).await {
        Either::Left(_) => info!(
            %context.our_id,
            "shutting down socket, no longer accepting incoming connections"
        ),
        Either::Right(_) => unreachable!(),
    }
}

/// Network message reader.
///
/// Schedules all received messages until the stream is closed or an error occurs.
pub(super) async fn message_reader<REv, P>(
    context: Arc<NetworkContext<REv>>,
    mut stream: SplitStream<FramedTransport<P>>,
    mut shutdown_receiver: watch::Receiver<()>,
    our_id: NodeId,
    peer_id: NodeId,
) -> io::Result<()>
where
    P: DeserializeOwned + Send + Display + Payload,
    REv: From<Event<P>>,
{
    let read_messages = async move {
        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(msg) => {
                    debug!(%msg, "message received");
                    // We've received a message, push it to the reactor.
                    context
                        .event_queue
                        .schedule(
                            Event::IncomingMessage {
                                peer_id: Box::new(peer_id),
                                msg: Box::new(msg),
                            },
                            QueueKind::NetworkIncoming,
                        )
                        .await;
                }
                Err(err) => {
                    warn!(%our_id, err=display_error(&err), %peer_id, "receiving message failed, closing connection");
                    return Err(err);
                }
            }
        }
        Ok(())
    };

    let shutdown_messages = async move { while shutdown_receiver.changed().await.is_ok() {} };

    // Now we can wait for either the `shutdown` channel's remote end to do be dropped or the
    // while loop to terminate.
    match future::select(Box::pin(shutdown_messages), Box::pin(read_messages)).await {
        Either::Left(_) => info!(
            %our_id,
            %peer_id,
            "shutting down incoming connection message reader"
        ),
        Either::Right(_) => (),
    }

    Ok(())
}

/// Network message sender.
///
/// Reads from a channel and sends all messages, until the stream is closed or an error occurs.
///
/// Initially sends a handshake including the `chainspec_hash` as a final handshake step.  If the
/// recipient's `chainspec_hash` doesn't match, the connection will be closed.
pub(super) async fn message_sender<P>(
    mut queue: UnboundedReceiver<Message<P>>,
    mut sink: SplitSink<FramedTransport<P>, Message<P>>,
    counter: IntGauge,
    handshake: Message<P>,
) -> Result<()>
where
    P: Serialize + Send + Payload,
{
    sink.send(handshake).await.map_err(Error::MessageNotSent)?;
    while let Some(payload) = queue.recv().await {
        counter.dec();
        // We simply error-out if the sink fails, it means that our connection broke.
        sink.send(payload).await.map_err(Error::MessageNotSent)?;
    }

    Ok(())
}
