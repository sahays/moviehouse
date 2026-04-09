#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("torrent parse error: {0}")]
    Metainfo(#[from] crate::torrent::metainfo::MetainfoError),
    #[error("magnet parse error: {0}")]
    Magnet(#[from] crate::torrent::magnet::MagnetError),
    #[error("bencode error: {0}")]
    Bencode(#[from] crate::bencode::DecodeError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tracker error: {0}")]
    Tracker(String),
    #[error("peer error: {0}")]
    Peer(String),
    #[error("{0}")]
    Other(String),
}
