use std::path::{Path, PathBuf};

use crate::torrent::metainfo::{FileLayout, Info};

/// A span within a single file where piece data should be read/written.
#[derive(Debug, Clone)]
pub struct FileSpan {
    pub file_index: usize,
    pub file_path: PathBuf,
    pub offset: u64,
    pub length: u64,
}

/// Maps piece indices to file locations on disk.
pub struct FileMapping {
    files: Vec<MappedFile>,
    piece_length: u64,
    total_length: u64,
}

struct MappedFile {
    path: PathBuf,
    offset_in_torrent: u64,
    length: u64,
}

impl FileMapping {
    pub fn new(info: &Info, output_dir: &Path) -> Self {
        let base_dir = output_dir.join(&info.name);
        let files = match &info.files {
            FileLayout::Single { length } => {
                vec![MappedFile {
                    path: base_dir,
                    offset_in_torrent: 0,
                    length: *length,
                }]
            }
            FileLayout::Multi { files } => {
                let mut offset = 0u64;
                files
                    .iter()
                    .map(|f| {
                        let mapped = MappedFile {
                            path: base_dir.join(&f.path),
                            offset_in_torrent: offset,
                            length: f.length,
                        };
                        offset += f.length;
                        mapped
                    })
                    .collect()
            }
        };

        Self {
            files,
            piece_length: info.piece_length as u64,
            total_length: info.total_length,
        }
    }

    /// Get the file spans that a piece covers.
    /// A single piece can span multiple files in a multi-file torrent.
    pub fn piece_spans(&self, piece_index: u32) -> Vec<FileSpan> {
        let piece_start = piece_index as u64 * self.piece_length;
        let piece_end = (piece_start + self.piece_length).min(self.total_length);
        let mut remaining = piece_end - piece_start;
        let mut current_offset = piece_start;
        let mut spans = Vec::new();

        for (i, file) in self.files.iter().enumerate() {
            let file_end = file.offset_in_torrent + file.length;
            if current_offset >= file_end {
                continue;
            }
            if current_offset < file.offset_in_torrent {
                break;
            }

            let offset_in_file = current_offset - file.offset_in_torrent;
            let available_in_file = file.length - offset_in_file;
            let span_length = remaining.min(available_in_file);

            spans.push(FileSpan {
                file_index: i,
                file_path: file.path.clone(),
                offset: offset_in_file,
                length: span_length,
            });

            current_offset += span_length;
            remaining -= span_length;

            if remaining == 0 {
                break;
            }
        }

        spans
    }

    /// Get all file paths.
    pub fn file_paths(&self) -> Vec<&Path> {
        self.files.iter().map(|f| f.path.as_path()).collect()
    }

    /// Get all files with their lengths for pre-allocation.
    pub fn files_for_allocation(&self) -> Vec<(&Path, u64)> {
        self.files.iter().map(|f| (f.path.as_path(), f.length)).collect()
    }
}
