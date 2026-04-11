use bytes::{Buf, BufMut, Bytes, BytesMut};

/// Standard block size: 16 KiB.
pub const BLOCK_SIZE: u32 = 16_384;

/// Peer wire protocol message.
#[derive(Debug, Clone)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        piece_index: u32,
    },
    Bitfield(Vec<u8>),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        data: Bytes,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    /// BEP10 extension message.
    Extended {
        id: u8,
        payload: Bytes,
    },
    /// BEP6: peer has ALL pieces (sent instead of Bitfield by seeders).
    HaveAll,
    /// BEP6: peer has NO pieces.
    HaveNone,
    /// Unknown message ID — skip gracefully.
    Unknown(u8),
}

// Message IDs
pub const MSG_CHOKE: u8 = 0;
pub const MSG_UNCHOKE: u8 = 1;
pub const MSG_INTERESTED: u8 = 2;
pub const MSG_NOT_INTERESTED: u8 = 3;
pub const MSG_HAVE: u8 = 4;
pub const MSG_BITFIELD: u8 = 5;
pub const MSG_REQUEST: u8 = 6;
pub const MSG_PIECE: u8 = 7;
pub const MSG_CANCEL: u8 = 8;
// BEP6 Fast Extension
pub const MSG_SUGGEST_PIECE: u8 = 13;
pub const MSG_HAVE_ALL: u8 = 14;
pub const MSG_HAVE_NONE: u8 = 15;
pub const MSG_REJECT_REQUEST: u8 = 16;
pub const MSG_ALLOWED_FAST: u8 = 17;
// BEP10
pub const MSG_EXTENDED: u8 = 20;

impl PeerMessage {
    /// Encode message to wire format (excluding the 4-byte length prefix — that's the codec's job).
    pub fn encode(&self, buf: &mut BytesMut) {
        match self {
            // KeepAlive has empty payload (codec adds length=0); Unknown is never sent
            PeerMessage::KeepAlive | PeerMessage::Unknown(_) => {}
            PeerMessage::Choke => buf.put_u8(MSG_CHOKE),
            PeerMessage::Unchoke => buf.put_u8(MSG_UNCHOKE),
            PeerMessage::Interested => buf.put_u8(MSG_INTERESTED),
            PeerMessage::NotInterested => buf.put_u8(MSG_NOT_INTERESTED),
            PeerMessage::Have { piece_index } => {
                buf.put_u8(MSG_HAVE);
                buf.put_u32(*piece_index);
            }
            PeerMessage::Bitfield(bitfield) => {
                buf.put_u8(MSG_BITFIELD);
                buf.extend_from_slice(bitfield);
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                buf.put_u8(MSG_REQUEST);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_u32(*length);
            }
            PeerMessage::Piece { index, begin, data } => {
                buf.put_u8(MSG_PIECE);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.extend_from_slice(data);
            }
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => {
                buf.put_u8(MSG_CANCEL);
                buf.put_u32(*index);
                buf.put_u32(*begin);
                buf.put_u32(*length);
            }
            PeerMessage::Extended { id, payload } => {
                buf.put_u8(MSG_EXTENDED);
                buf.put_u8(*id);
                buf.extend_from_slice(payload);
            }
            PeerMessage::HaveAll => buf.put_u8(MSG_HAVE_ALL),
            PeerMessage::HaveNone => buf.put_u8(MSG_HAVE_NONE),
        }
    }

    /// Decode a message from wire bytes (excluding length prefix).
    /// `data` is the full message payload after the 4-byte length prefix.
    pub fn decode(data: &mut BytesMut) -> Result<Self, PeerMessageError> {
        if data.is_empty() {
            return Ok(PeerMessage::KeepAlive);
        }

        let id = data.get_u8();
        match id {
            MSG_CHOKE => Ok(PeerMessage::Choke),
            MSG_UNCHOKE => Ok(PeerMessage::Unchoke),
            MSG_INTERESTED => Ok(PeerMessage::Interested),
            MSG_NOT_INTERESTED => Ok(PeerMessage::NotInterested),
            MSG_HAVE => {
                ensure_len(data, 4)?;
                Ok(PeerMessage::Have {
                    piece_index: data.get_u32(),
                })
            }
            MSG_BITFIELD => {
                let bitfield = data.to_vec();
                data.clear();
                Ok(PeerMessage::Bitfield(bitfield))
            }
            MSG_REQUEST => {
                ensure_len(data, 12)?;
                Ok(PeerMessage::Request {
                    index: data.get_u32(),
                    begin: data.get_u32(),
                    length: data.get_u32(),
                })
            }
            MSG_PIECE => {
                ensure_len(data, 8)?;
                let index = data.get_u32();
                let begin = data.get_u32();
                let piece_data = data.split().freeze();
                Ok(PeerMessage::Piece {
                    index,
                    begin,
                    data: piece_data,
                })
            }
            MSG_CANCEL => {
                ensure_len(data, 12)?;
                Ok(PeerMessage::Cancel {
                    index: data.get_u32(),
                    begin: data.get_u32(),
                    length: data.get_u32(),
                })
            }
            MSG_EXTENDED => {
                ensure_len(data, 1)?;
                let ext_id = data.get_u8();
                let payload = data.split().freeze();
                Ok(PeerMessage::Extended {
                    id: ext_id,
                    payload,
                })
            }
            MSG_HAVE_ALL => Ok(PeerMessage::HaveAll),
            MSG_HAVE_NONE => Ok(PeerMessage::HaveNone),
            // BEP6: Suggest, Reject, Allowed Fast, and any unknown message — skip gracefully
            _ => {
                data.clear();
                Ok(PeerMessage::Unknown(id))
            }
        }
    }

    /// Wire length of this message (excluding 4-byte length prefix).
    pub fn wire_len(&self) -> usize {
        match self {
            PeerMessage::KeepAlive => 0,
            PeerMessage::Choke
            | PeerMessage::Unchoke
            | PeerMessage::Interested
            | PeerMessage::NotInterested
            | PeerMessage::HaveAll
            | PeerMessage::HaveNone
            | PeerMessage::Unknown(_) => 1,
            PeerMessage::Have { .. } => 5,
            PeerMessage::Bitfield(bf) => 1 + bf.len(),
            PeerMessage::Request { .. } | PeerMessage::Cancel { .. } => 13,
            PeerMessage::Piece { data, .. } => 9 + data.len(),
            PeerMessage::Extended { payload, .. } => 2 + payload.len(),
        }
    }
}

fn ensure_len(data: &BytesMut, min: usize) -> Result<(), PeerMessageError> {
    if data.remaining() < min {
        Err(PeerMessageError::TooShort)
    } else {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PeerMessageError {
    #[error("unknown message id: {0}")]
    UnknownMessageId(u8),
    #[error("message too short")]
    TooShort,
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
}
