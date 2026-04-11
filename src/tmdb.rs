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
    overview: Option<String>,
    poster_path: Option<String>,
    vote_average: Option<f32>,
    release_date: Option<String>,
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
        url.push_str(&format!("&year={y}"));
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
        title: movie.title.clone(),
        poster_url,
        overview,
        rating,
        cast,
        director,
        year,
    })
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
