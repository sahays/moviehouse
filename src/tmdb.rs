use serde::Deserialize;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500";

#[derive(Debug, Clone, Deserialize)]
struct SearchResult {
    results: Vec<TmdbMovie>,
}

#[derive(Debug, Clone, Deserialize)]
struct TmdbMovie {
    id: u64,
    #[allow(dead_code)]
    title: Option<String>,
    #[allow(dead_code)]
    name: Option<String>, // TV shows use "name" instead of "title"
    overview: Option<String>,
    poster_path: Option<String>,
    vote_average: Option<f32>,
    release_date: Option<String>,
    first_air_date: Option<String>, // TV shows
}

#[derive(Debug, Clone, Deserialize)]
struct CreditsResponse {
    cast: Option<Vec<CastMember>>,
    crew: Option<Vec<CrewMember>>,
}

#[derive(Debug, Clone, Deserialize)]
struct CastMember {
    name: String,
    order: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct CrewMember {
    name: String,
    job: String,
}

#[derive(Debug, Clone)]
pub struct MovieMetadata {
    pub tmdb_id: u64,
    pub title: Option<String>,
    pub poster_url: Option<String>,
    pub overview: Option<String>,
    pub rating: Option<f32>,
    pub cast: Vec<String>,
    pub director: Option<String>,
    pub year: Option<u16>,
}

/// Search TMDB for a movie by title and optional year.
/// Returns metadata if found.
pub async fn fetch_metadata(
    api_key: &str,
    title: &str,
    year: Option<u16>,
) -> Option<MovieMetadata> {
    let client = reqwest::Client::new();

    // Search for the movie
    let mut url = format!(
        "{TMDB_BASE}/search/movie?api_key={api_key}&query={}",
        urlencoding(title)
    );
    if let Some(y) = year {
        use std::fmt::Write;
        let _ = write!(url, "&year={y}");
    }

    let search: SearchResult = client.get(&url).send().await.ok()?.json().await.ok()?;

    let movie = search.results.first()?;
    let movie_id = movie.id;

    let poster_url = movie
        .poster_path
        .as_ref()
        .map(|p| format!("{IMAGE_BASE}{p}"));
    let overview = movie.overview.clone();
    let rating = movie.vote_average;
    let year = movie
        .release_date
        .as_ref()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<u16>().ok());

    // Fetch credits for cast + director
    let credits_url = format!("{TMDB_BASE}/movie/{movie_id}/credits?api_key={api_key}");
    let credits: CreditsResponse = client
        .get(&credits_url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let cast: Vec<String> = credits
        .cast
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.order.unwrap_or(999) < 5) // top 5 cast
        .map(|c| c.name)
        .collect();

    let director = credits
        .crew
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.job == "Director")
        .map(|c| c.name);

    Some(MovieMetadata {
        tmdb_id: movie_id,
        title: movie.title.clone(),
        poster_url,
        overview,
        rating,
        cast,
        director,
        year,
    })
}

/// Search TMDB for a TV show by title.
/// Returns metadata if found.
pub async fn fetch_tv_metadata(api_key: &str, title: &str) -> Option<MovieMetadata> {
    let client = reqwest::Client::new();

    let url = format!(
        "{TMDB_BASE}/search/tv?api_key={api_key}&query={}",
        urlencoding(title)
    );

    let search: SearchResult = client.get(&url).send().await.ok()?.json().await.ok()?;
    let show = search.results.first()?;
    let show_id = show.id;

    let poster_url = show
        .poster_path
        .as_ref()
        .map(|p| format!("{IMAGE_BASE}{p}"));
    let overview = show.overview.clone();
    let rating = show.vote_average;
    let year = show
        .first_air_date
        .as_ref()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<u16>().ok());

    // Fetch credits for cast + director (creator for TV)
    let credits_url = format!("{TMDB_BASE}/tv/{show_id}/credits?api_key={api_key}");
    let credits: CreditsResponse = client
        .get(&credits_url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let cast: Vec<String> = credits
        .cast
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.order.unwrap_or(999) < 5)
        .map(|c| c.name)
        .collect();

    let director = credits
        .crew
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.job == "Director" || c.job == "Executive Producer")
        .map(|c| c.name);

    Some(MovieMetadata {
        tmdb_id: show_id,
        title: show.name.clone().or_else(|| show.title.clone()),
        poster_url,
        overview,
        rating,
        cast,
        director,
        year,
    })
}

/// Auto-select movie vs TV search based on `is_show`.
pub async fn fetch_metadata_auto(
    api_key: &str,
    title: &str,
    year: Option<u16>,
    is_show: bool,
) -> Option<MovieMetadata> {
    if is_show {
        fetch_tv_metadata(api_key, title).await
    } else {
        fetch_metadata(api_key, title, year).await
    }
}

/// Per-episode metadata from TMDB.
#[derive(Debug, Clone)]
pub struct EpisodeMetadata {
    pub name: String,
    pub overview: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TmdbSeasonResponse {
    episodes: Option<Vec<TmdbEpisode>>,
}

#[derive(Debug, Clone, Deserialize)]
struct TmdbEpisode {
    episode_number: u16,
    name: Option<String>,
    overview: Option<String>,
}

/// Fetch per-episode metadata for a TV season.
/// If `tmdb_id` is provided, uses it directly. Otherwise searches by `show_title`.
/// Returns a map of `episode_number` → `EpisodeMetadata`.
pub async fn fetch_season_episodes(
    api_key: &str,
    tmdb_id: Option<u64>,
    show_title: &str,
    season: u16,
) -> Option<std::collections::HashMap<u16, EpisodeMetadata>> {
    let client = reqwest::Client::new();

    // Use stored TMDB ID if available, otherwise search by name
    let show_id = if let Some(id) = tmdb_id {
        id
    } else {
        let url = format!(
            "{TMDB_BASE}/search/tv?api_key={api_key}&query={}",
            urlencoding(show_title)
        );
        let search: SearchResult = client.get(&url).send().await.ok()?.json().await.ok()?;
        search.results.first()?.id
    };

    // Fetch season data using the ID directly
    let season_url = format!("{TMDB_BASE}/tv/{show_id}/season/{season}?api_key={api_key}");
    let season_data: TmdbSeasonResponse = client
        .get(&season_url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let mut episodes = std::collections::HashMap::new();
    for ep in season_data.episodes.unwrap_or_default() {
        episodes.insert(
            ep.episode_number,
            EpisodeMetadata {
                name: ep.name.unwrap_or_default(),
                overview: ep.overview.unwrap_or_default(),
            },
        );
    }

    Some(episodes)
}

/// Apply TMDB metadata to a `MediaEntry`.
/// Writes poster, overview, rating, cast, director, `tmdb_id`, and conditionally title/year.
pub fn apply_metadata(entry: &mut crate::engine::types::MediaEntry, meta: &MovieMetadata) {
    if let Some(ref title) = meta.title {
        title.clone_into(&mut entry.title);
    }
    entry.poster_url.clone_from(&meta.poster_url);
    entry.overview.clone_from(&meta.overview);
    entry.rating = meta.rating;
    entry.cast.clone_from(&meta.cast);
    entry.director.clone_from(&meta.director);
    entry.tmdb_id = Some(meta.tmdb_id);
    if meta.year.is_some() && entry.year.is_none() {
        entry.year = meta.year;
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else if c == ' ' {
                "+".to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}
