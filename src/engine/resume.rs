//! Rebuilding a download's progress from the files already on disk.
//!
//! Sessions are pure in-memory state: `TorrentSession::new` starts at zero
//! pieces. Without this, restarting the server threw away every byte of an
//! unfinished download and re-fetched it from the network.
//!
//! `DownloadRecord::completed_pieces` exists but is never written (nothing calls
//! `Store::update_pieces`), so a persisted bitfield cannot be trusted as the
//! source of truth. The files themselves are, and they are self-verifying: every
//! piece is checked against the SHA-1 in the metainfo. That makes this robust to
//! a stale record, a half-written piece from a crash or a full disk, and files
//! edited or removed behind our back — anything that fails the hash is simply
//! re-downloaded.
//!
//! The cost is one sequential read of whatever is on disk. That is I/O-bound and
//! takes minutes for a large torrent, but it transfers nothing over the network.

use crate::disk::io::DiskHandle;
use crate::piece::picker::PiecePicker;
use crate::piece::store::PieceStore;
use crate::torrent::metainfo::Info;

/// Log progress every this many pieces — a large verify is otherwise silent.
const LOG_EVERY: usize = 500;

/// Length of `piece_index`, accounting for the short final piece.
fn piece_len(info: &Info, piece_index: usize) -> u32 {
    if piece_index + 1 == info.pieces.len() {
        let remainder = (info.total_length % u64::from(info.piece_length)) as u32;
        if remainder == 0 {
            info.piece_length
        } else {
            remainder
        }
    } else {
        info.piece_length
    }
}

/// Hash every piece present on disk and mark the good ones complete in `picker`.
///
/// Returns the number of pieces adopted. A piece that cannot be read, or whose
/// data does not match its SHA-1, is left unset and will be re-requested — so
/// this can only ever under-claim, never corrupt the download.
pub async fn verify_on_disk(
    disk: &DiskHandle,
    piece_store: &PieceStore,
    info: &Info,
    picker: &mut PiecePicker,
) -> usize {
    let num_pieces = info.pieces.len();
    let mut verified = 0usize;

    for index in 0..num_pieces {
        let length = piece_len(info, index);
        let Ok(data) = disk.read_piece(index as u32, length).await else {
            continue;
        };
        if data.len() as u32 == length && piece_store.verify(index as u32, &data) {
            picker.mark_verified(index as u32);
            verified += 1;
        }

        if index > 0 && index % LOG_EVERY == 0 {
            tracing::info!(
                checked = index,
                total = num_pieces,
                verified,
                "verifying existing data"
            );
        }
    }

    verified
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test fixtures: a failed setup should fail the test loudly
mod tests {
    use super::*;

    use crate::torrent::metainfo::FileLayout;
    use crate::torrent::types::Sha1Hash;

    fn info(piece_length: u32, total_length: u64, pieces: usize) -> Info {
        Info {
            piece_length,
            pieces: vec![Sha1Hash([0u8; 20]); pieces],
            name: "t".into(),
            files: FileLayout::Single {
                length: total_length,
            },
            total_length,
        }
    }

    #[test]
    fn non_final_pieces_are_full_length() {
        let i = info(1024, 4096, 4);
        assert_eq!(piece_len(&i, 0), 1024);
        assert_eq!(piece_len(&i, 2), 1024);
    }

    #[test]
    fn final_piece_is_the_remainder() {
        // 3 full pieces + 100 bytes.
        let i = info(1024, 3 * 1024 + 100, 4);
        assert_eq!(piece_len(&i, 3), 100);
    }

    #[test]
    fn final_piece_is_full_when_evenly_divisible() {
        let i = info(1024, 4096, 4);
        assert_eq!(piece_len(&i, 3), 1024);
    }

    // ── End-to-end: real file on disk → real hashes → adopted pieces ──
    //
    // This is the load-bearing behaviour. If it regresses, restarting the server
    // silently re-downloads torrents that are already complete on disk.

    use crate::disk::io::create_disk_manager;
    use crate::disk::mapping::FileMapping;
    use sha1::{Digest, Sha1};
    use tokio_util::sync::CancellationToken;

    /// A single-file torrent whose piece hashes match `content`.
    fn info_for(content: &[u8], piece_length: u32, name: &str) -> Info {
        let pieces = content
            .chunks(piece_length as usize)
            .map(|chunk| {
                let mut h = [0u8; 20];
                h.copy_from_slice(&Sha1::digest(chunk));
                Sha1Hash(h)
            })
            .collect();
        Info {
            piece_length,
            pieces,
            name: name.into(),
            files: FileLayout::Single {
                length: content.len() as u64,
            },
            total_length: content.len() as u64,
        }
    }

    /// Run `verify_on_disk` against `on_disk`, for a torrent describing `truth`.
    async fn adopted(truth: &[u8], on_disk: &[u8], case: &str) -> usize {
        let dir = std::env::temp_dir().join(format!("mh-resume-{case}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let info = info_for(truth, 1024, "payload.bin");
        std::fs::write(dir.join("payload.bin"), on_disk).unwrap();

        let cancel = CancellationToken::new();
        let (disk, manager) = create_disk_manager(FileMapping::new(&info, &dir), cancel, false);
        let task = tokio::spawn(async move { manager.run().await });

        let piece_store = PieceStore::new(info.pieces.clone());
        let mut picker = PiecePicker::new(info.pieces.len(), info.piece_length, info.total_length);
        let n = verify_on_disk(&disk, &piece_store, &info, &mut picker).await;

        assert_eq!(n, picker.pieces_done(), "picker must agree with the count");
        drop(disk);
        task.abort();
        let _ = std::fs::remove_dir_all(&dir);
        n
    }

    #[tokio::test]
    async fn adopts_every_piece_of_a_complete_file() {
        // 4 full pieces + a short final one.
        let content: Vec<u8> = (0..4 * 1024 + 300).map(|i| (i % 251) as u8).collect();
        assert_eq!(adopted(&content, &content, "complete").await, 5);
    }

    #[tokio::test]
    async fn adopts_nothing_when_the_data_is_wrong() {
        let content: Vec<u8> = (0..4 * 1024).map(|i| (i % 251) as u8).collect();
        let garbage = vec![0u8; content.len()];
        assert_eq!(adopted(&content, &garbage, "garbage").await, 0);
    }

    #[tokio::test]
    async fn adopts_only_the_intact_pieces_of_a_partial_file() {
        // Correct first two pieces, corrupted third — the shape left behind by a
        // download that died mid-piece or ran out of disk.
        let content: Vec<u8> = (0..3 * 1024).map(|i| (i % 251) as u8).collect();
        let mut damaged = content.clone();
        damaged[2 * 1024 + 5] ^= 0xFF;
        assert_eq!(adopted(&content, &damaged, "partial").await, 2);
    }
}
