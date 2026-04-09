use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::codec::Framed;
use tracing::debug;

use super::codec::{PeerCodec, PeerCodecError};
use super::extension::ExtendedHandshake;
use super::handshake::Handshake;
use super::message::PeerMessage;
use crate::torrent::types::{InfoHash, PeerId};

/// Events from a peer connection to the session coordinator.
#[derive(Debug)]
pub enum PeerEvent {
    Connected {
        addr: SocketAddr,
        bitfield: Option<Vec<u8>>,
        supports_extensions: bool,
    },
    Unchoked,
    Choked,
    Have {
        piece_index: u32,
    },
    BitfieldReceived(Vec<u8>),
    /// BEP6: peer has ALL pieces (seeder).
    HaveAll,
    BlockReceived {
        piece_index: u32,
        offset: u32,
        data: Bytes,
    },
    ExtendedHandshake(ExtendedHandshake),
    MetadataMessage(super::extension::MetadataMessage),
    /// BEP11: Peer Exchange -- new peers discovered via PEX.
    PexPeers(Vec<SocketAddr>),
    Disconnected {
        reason: String,
    },
}

/// Commands from the session coordinator to a peer connection.
#[derive(Debug)]
pub enum PeerCommand {
    RequestBlock {
        index: u32,
        begin: u32,
        length: u32,
    },
    CancelBlock {
        index: u32,
        begin: u32,
        length: u32,
    },
    SendInterested,
    SendNotInterested,
    SendChoke,
    SendUnchoke,
    SendHave {
        piece_index: u32,
    },
    SendExtendedHandshake(ExtendedHandshake),
    SendMetadataRequest {
        ext_id: u8,
        piece: u32,
    },
    Disconnect,
}

/// Run a peer connection as an async task.
///
/// Performs handshake, then processes messages bidirectionally
/// until disconnect or cancellation.
pub async fn run_peer_connection(
    addr: SocketAddr,
    info_hash: InfoHash,
    our_peer_id: PeerId,
    event_tx: mpsc::Sender<(SocketAddr, PeerEvent)>,
    mut cmd_rx: mpsc::Receiver<PeerCommand>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let result = run_inner(addr, info_hash, our_peer_id, &event_tx, &mut cmd_rx, &cancel).await;

    let reason = match result {
        Ok(()) => "clean disconnect".to_string(),
        Err(e) => format!("{e}"),
    };

    let _ = event_tx
        .send((addr, PeerEvent::Disconnected { reason }))
        .await;
}

async fn run_inner(
    addr: SocketAddr,
    info_hash: InfoHash,
    our_peer_id: PeerId,
    event_tx: &mpsc::Sender<(SocketAddr, PeerEvent)>,
    cmd_rx: &mut mpsc::Receiver<PeerCommand>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), PeerConnectionError> {
    // Connect with timeout
    let stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr))
        .await
        .map_err(|_| PeerConnectionError::ConnectTimeout)?
        .map_err(PeerConnectionError::Io)?;

    // Tune socket
    configure_socket(&stream);

    let mut stream_raw = stream;

    // Send our handshake
    let our_handshake = Handshake::new(info_hash, our_peer_id);
    our_handshake
        .write_to(&mut stream_raw)
        .await
        .map_err(PeerConnectionError::Io)?;

    // Read peer's handshake
    let peer_handshake = tokio::time::timeout(
        Duration::from_secs(10),
        Handshake::read_from(&mut stream_raw),
    )
    .await
    .map_err(|_| PeerConnectionError::HandshakeTimeout)?
    .map_err(PeerConnectionError::Handshake)?;

    // Verify info_hash
    if peer_handshake.info_hash != info_hash {
        return Err(PeerConnectionError::InfoHashMismatch);
    }

    let supports_extensions = peer_handshake.supports_extension_protocol();

    debug!(
        peer = %addr,
        peer_id = ?peer_handshake.peer_id,
        extensions = supports_extensions,
        "Handshake complete"
    );

    // Wrap in Framed codec
    let mut framed = Framed::new(stream_raw, PeerCodec);

    // Send connected event (we'll get the bitfield separately)
    let _ = event_tx
        .send((
            addr,
            PeerEvent::Connected {
                addr,
                bitfield: None,
                supports_extensions,
            },
        ))
        .await;

    // Main message loop
    let mut keepalive_interval = tokio::time::interval(Duration::from_secs(60));
    keepalive_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!(peer = %addr, "Peer connection cancelled");
                break;
            }

            // Incoming messages from peer
            frame = framed.next() => {
                match frame {
                    Some(Ok(msg)) => {
                        handle_incoming_message(addr, msg, event_tx, supports_extensions).await?;
                    }
                    Some(Err(e)) => {
                        return Err(PeerConnectionError::Codec(e));
                    }
                    None => {
                        // Connection closed
                        break;
                    }
                }
            }

            // Outgoing commands from session
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(cmd) => {
                        if let Err(e) = handle_command(&mut framed, cmd).await {
                            return Err(e);
                        }
                    }
                    None => {
                        // Command channel closed, disconnect
                        break;
                    }
                }
            }

            // Send keepalive
            _ = keepalive_interval.tick() => {
                if let Err(e) = framed.send(PeerMessage::KeepAlive).await {
                    return Err(PeerConnectionError::Codec(e));
                }
            }
        }
    }

    Ok(())
}

async fn handle_incoming_message(
    addr: SocketAddr,
    msg: PeerMessage,
    event_tx: &mpsc::Sender<(SocketAddr, PeerEvent)>,
    supports_extensions: bool,
) -> Result<(), PeerConnectionError> {
    let event = match msg {
        PeerMessage::KeepAlive => return Ok(()),
        PeerMessage::Choke => PeerEvent::Choked,
        PeerMessage::Unchoke => PeerEvent::Unchoked,
        PeerMessage::Interested | PeerMessage::NotInterested => return Ok(()),
        PeerMessage::Have { piece_index } => PeerEvent::Have { piece_index },
        PeerMessage::Bitfield(bf) => PeerEvent::BitfieldReceived(bf),
        PeerMessage::HaveAll => PeerEvent::HaveAll,
        PeerMessage::HaveNone | PeerMessage::Unknown(_) => return Ok(()),
        PeerMessage::Piece { index, begin, data } => PeerEvent::BlockReceived {
            piece_index: index,
            offset: begin,
            data,
        },
        PeerMessage::Request { .. } | PeerMessage::Cancel { .. } => return Ok(()),
        PeerMessage::Extended { id, payload } => {
            if !supports_extensions {
                return Ok(());
            }
            match dispatch_extension(id, &payload) {
                Some(event) => event,
                None => return Ok(()),
            }
        }
    };

    let _ = event_tx.send((addr, event)).await;
    Ok(())
}

/// Dispatch an extension message.
///
/// BEP10: peers send messages using OUR extension IDs (which we advertised
/// in our handshake). We always advertise ut_metadata=1, ut_pex=2.
/// ID 0 is always the extended handshake itself.
fn dispatch_extension(id: u8, payload: &[u8]) -> Option<PeerEvent> {
    match id {
        0 => {
            let hs = ExtendedHandshake::from_bencode(payload).ok()?;
            Some(PeerEvent::ExtendedHandshake(hs))
        }
        1 => {
            // ut_metadata — our ID 1
            let msg = super::extension::MetadataMessage::from_bytes(payload).ok()?;
            Some(PeerEvent::MetadataMessage(msg))
        }
        2 => {
            // ut_pex — our ID 2
            let pex = super::extension::PexMessage::from_bencode(payload).ok()?;
            if pex.added.is_empty() { None } else { Some(PeerEvent::PexPeers(pex.added)) }
        }
        _ => None, // Extension ID we didn't advertise
    }
}

async fn handle_command(
    framed: &mut Framed<TcpStream, PeerCodec>,
    cmd: PeerCommand,
) -> Result<(), PeerConnectionError> {
    let msg = match cmd {
        PeerCommand::RequestBlock {
            index,
            begin,
            length,
        } => PeerMessage::Request {
            index,
            begin,
            length,
        },
        PeerCommand::CancelBlock {
            index,
            begin,
            length,
        } => PeerMessage::Cancel {
            index,
            begin,
            length,
        },
        PeerCommand::SendInterested => PeerMessage::Interested,
        PeerCommand::SendNotInterested => PeerMessage::NotInterested,
        PeerCommand::SendChoke => PeerMessage::Choke,
        PeerCommand::SendUnchoke => PeerMessage::Unchoke,
        PeerCommand::SendHave { piece_index } => PeerMessage::Have { piece_index },
        PeerCommand::SendExtendedHandshake(hs) => PeerMessage::Extended {
            id: 0,
            payload: Bytes::from(hs.to_bencode()),
        },
        PeerCommand::SendMetadataRequest { ext_id, piece } => {
            let msg =
                super::extension::MetadataMessage::Request { piece };
            PeerMessage::Extended {
                id: ext_id,
                payload: Bytes::from(msg.to_bytes()),
            }
        }
        PeerCommand::Disconnect => return Ok(()),
    };

    framed
        .send(msg)
        .await
        .map_err(PeerConnectionError::Codec)?;
    Ok(())
}

fn configure_socket(stream: &TcpStream) {
    let socket = socket2::SockRef::from(stream);

    // TCP_NODELAY: critical for request pipelining
    let _ = socket.set_nodelay(true);

    // Increase receive buffer to 2 MiB
    let _ = socket.set_recv_buffer_size(2 * 1024 * 1024);

    // Increase send buffer to 1 MiB
    let _ = socket.set_send_buffer_size(1 * 1024 * 1024);
}

#[derive(Debug, thiserror::Error)]
pub enum PeerConnectionError {
    #[error("connect timeout")]
    ConnectTimeout,
    #[error("handshake timeout")]
    HandshakeTimeout,
    #[error("info hash mismatch")]
    InfoHashMismatch,
    #[error("handshake error: {0}")]
    Handshake(super::handshake::HandshakeError),
    #[error("codec error: {0}")]
    Codec(PeerCodecError),
    #[error("IO error: {0}")]
    Io(std::io::Error),
}
