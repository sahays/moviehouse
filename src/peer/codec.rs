use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use super::message::{PeerMessage, PeerMessageError};

/// Maximum frame size: 16 KiB block + 9 bytes overhead for Piece message + 4 bytes length prefix.
/// With some margin for extension messages.
const MAX_FRAME_SIZE: usize = 1 << 20; // 1 MiB

/// Length-prefixed message codec for the peer wire protocol.
///
/// Wire format: <4-byte big-endian length><payload>
/// `KeepAlive`: length=0, no payload.
pub struct PeerCodec;

impl Decoder for PeerCodec {
    type Item = PeerMessage;
    type Error = PeerCodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Need at least 4 bytes for the length prefix
        if src.len() < 4 {
            return Ok(None);
        }

        // Peek at the length without consuming
        let length = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;

        if length > MAX_FRAME_SIZE {
            return Err(PeerCodecError::FrameTooLarge(length));
        }

        // Check if we have the full frame
        if src.len() < 4 + length {
            // Reserve space for the rest of the frame
            src.reserve(4 + length - src.len());
            return Ok(None);
        }

        // Consume the length prefix
        src.advance(4);

        // Extract the message payload
        let mut payload = src.split_to(length);

        let msg = PeerMessage::decode(&mut payload).map_err(PeerCodecError::Message)?;
        Ok(Some(msg))
    }
}

impl Encoder<PeerMessage> for PeerCodec {
    type Error = PeerCodecError;

    fn encode(&mut self, msg: PeerMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let len = msg.wire_len();
        if len > MAX_FRAME_SIZE {
            return Err(PeerCodecError::FrameTooLarge(len));
        }
        dst.reserve(4 + len);

        // Write length prefix
        dst.put_u32(len as u32);

        // Write message payload
        msg.encode(dst);

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PeerCodecError {
    #[error("frame too large: {0} bytes")]
    FrameTooLarge(usize),
    #[error("message error: {0}")]
    Message(PeerMessageError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_encode_oversized_message() {
        let mut codec = PeerCodec;
        let mut buf = BytesMut::new();
        // Create a piece message larger than MAX_FRAME_SIZE (1 MiB)
        let big_data = vec![0u8; MAX_FRAME_SIZE + 1];
        let msg = PeerMessage::Piece {
            index: 0,
            begin: 0,
            data: Bytes::from(big_data),
        };
        let result = codec.encode(msg, &mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_encode_normal_message() {
        let mut codec = PeerCodec;
        let mut buf = BytesMut::new();
        let msg = PeerMessage::Piece {
            index: 0,
            begin: 0,
            data: Bytes::from(vec![0u8; 16384]),
        };
        assert!(codec.encode(msg, &mut buf).is_ok());
        assert!(!buf.is_empty());
    }
}
