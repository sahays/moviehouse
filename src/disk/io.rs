use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::mapping::FileMapping;

/// Commands sent to the disk I/O task.
pub enum DiskCommand {
    WritePiece {
        piece_index: u32,
        data: Vec<u8>,
        response: oneshot::Sender<DiskResult>,
    },
    ReadPiece {
        piece_index: u32,
        length: u32,
        response: oneshot::Sender<Result<Vec<u8>, std::io::Error>>,
    },
    PreAllocate,
    Shutdown,
}

#[derive(Debug)]
pub enum DiskResult {
    Ok { piece_index: u32 },
    Error { piece_index: u32, error: String },
}

/// Disk I/O manager. Runs as a single task processing a bounded channel.
pub struct DiskManager {
    mapping: FileMapping,
    cmd_rx: mpsc::Receiver<DiskCommand>,
    cancel: CancellationToken,
    lightspeed: bool,
}

/// Handle for sending commands to the disk manager.
#[derive(Clone)]
pub struct DiskHandle {
    cmd_tx: mpsc::Sender<DiskCommand>,
}

impl DiskHandle {
    /// Write a verified piece to disk.
    pub async fn write_piece(
        &self,
        piece_index: u32,
        data: Vec<u8>,
    ) -> Result<DiskResult, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(DiskCommand::WritePiece {
                piece_index,
                data,
                response: tx,
            })
            .await
            .map_err(|e| format!("send failed: {e}"))?;
        match rx.await {
            Ok(result) => Ok(result),
            Err(_) => Ok(DiskResult::Error {
                piece_index,
                error: "disk manager channel closed".into(),
            }),
        }
    }

    /// Read a piece from disk (for seeding or resume verification).
    pub async fn read_piece(
        &self,
        piece_index: u32,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(DiskCommand::ReadPiece {
                piece_index,
                length,
                response: tx,
            })
            .await
            .map_err(|e| e.to_string())?;
        rx.await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Pre-allocate all files.
    pub async fn pre_allocate(&self) {
        let _ = self.cmd_tx.send(DiskCommand::PreAllocate).await;
    }
}

pub fn create_disk_manager(
    mapping: FileMapping,
    cancel: CancellationToken,
    lightspeed: bool,
) -> (DiskHandle, DiskManager) {
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let handle = DiskHandle { cmd_tx };
    let manager = DiskManager {
        mapping,
        cmd_rx,
        cancel,
        lightspeed,
    };
    (handle, manager)
}

impl DiskManager {
    /// Run the disk I/O event loop. Should be spawned with `tokio::task::spawn_blocking`
    /// or as a regular tokio task that uses `spawn_blocking` internally.
    pub async fn run(mut self) {
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    debug!("Disk manager received cancel, draining remaining writes...");
                    // Drain any remaining commands in the channel before exiting
                    self.drain_remaining().await;
                    break;
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(cmd) => self.process_command(cmd).await,
                        None => break,
                    }
                }
            }
        }
        debug!("Disk manager shut down");
    }

    async fn drain_remaining(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            // Always sync on drain (safety on shutdown)
            self.process_command_with_sync(cmd, true).await;
        }
    }

    async fn process_command(&self, cmd: DiskCommand) {
        let sync = !self.lightspeed;
        self.process_command_with_sync(cmd, sync).await;
    }

    async fn process_command_with_sync(&self, cmd: DiskCommand, sync: bool) {
        match cmd {
            DiskCommand::WritePiece { piece_index, data, response } => {
                let spans = self.mapping.piece_spans(piece_index);
                let result = tokio::task::spawn_blocking(move || {
                    write_piece_blocking(&spans, &data, sync)
                }).await;

                let disk_result = match result {
                    Ok(Ok(())) => DiskResult::Ok { piece_index },
                    Ok(Err(e)) => {
                        error!(piece = piece_index, error = %e, "Disk write failed");
                        DiskResult::Error { piece_index, error: e.to_string() }
                    }
                    Err(e) => {
                        DiskResult::Error { piece_index, error: e.to_string() }
                    }
                };
                let _ = response.send(disk_result);
            }
            DiskCommand::ReadPiece { piece_index, length, response } => {
                let spans = self.mapping.piece_spans(piece_index);
                let result = tokio::task::spawn_blocking(move || {
                    read_piece_blocking(&spans, length)
                }).await;

                let _ = response.send(result.unwrap_or_else(|e| {
                    Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                }));
            }
            DiskCommand::PreAllocate => {
                let files = self.mapping.files_for_allocation();
                for (path, length) in files {
                    if let Err(e) = pre_allocate_file(path, length) {
                        warn!(path = %path.display(), error = %e, "Pre-allocation failed");
                    }
                }
                info!("File pre-allocation complete");
            }
            DiskCommand::Shutdown => {}
        }
    }
}

use super::mapping::FileSpan;

fn write_piece_blocking(spans: &[FileSpan], data: &[u8], sync: bool) -> Result<(), std::io::Error> {
    let mut data_offset = 0usize;
    for span in spans {
        // Ensure parent directory exists
        if let Some(parent) = span.file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&span.file_path)?;

        file.seek(SeekFrom::Start(span.offset))?;
        let end = data_offset + span.length as usize;
        file.write_all(&data[data_offset..end])?;
        if sync {
            file.sync_all()?;
        }

        data_offset = end;
    }
    Ok(())
}

fn read_piece_blocking(spans: &[FileSpan], total_length: u32) -> Result<Vec<u8>, std::io::Error> {
    let mut data = vec![0u8; total_length as usize];
    let mut data_offset = 0usize;

    for span in spans {
        let mut file = File::open(&span.file_path)?;
        file.seek(SeekFrom::Start(span.offset))?;
        let end = data_offset + span.length as usize;
        file.read_exact(&mut data[data_offset..end])?;
        data_offset = end;
    }

    Ok(data)
}

fn pre_allocate_file(path: &Path, length: u64) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(path)?;

    file.set_len(length)?;
    debug!(path = %path.display(), length, "Pre-allocated file");
    Ok(())
}
