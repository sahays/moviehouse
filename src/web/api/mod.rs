pub mod filesystem;
pub mod library;
pub mod media;
pub mod settings;
pub mod torrents;
pub mod transcode;

use serde::Serialize;

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
}

#[derive(serde::Deserialize)]
pub struct SeasonQuery {
    pub season: Option<u16>,
}
