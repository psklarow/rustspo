// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paweł Sklarow
//
// Command-line entry point for rustspo.
// Coordinates CLI parsing, cache persistence, playlist loading, and queue updates.

mod spotify;
mod http_listener;

use serde::{Serialize, Deserialize};
use clap::Parser;
use futures::StreamExt;

#[derive(Parser, Debug)]
struct Cli {
    /// Name of the playlist to shuffle. The playlist must be in the user's library. Playlist "All Songs" represents all songs in the user's library.
    playlist_name: String,

    /// Number of songs to shuffle
    #[arg(short('n'), default_value = "20")]
    number_of_songs: usize,

    /// If true, the program will force reauthentication with Spotify API, ignoring any cached access token. Could be useful when permissions changed or saved access token is malformed.
    #[arg(short('r'), long("reauthenticate"))]
    reauthenticate: bool,

    /// If true, the program will not actually add songs to the queue, but will print which songs would be added. Useful for testing and debugging.
    #[arg(short('d'), long("dry-run"))]
    dry_run: bool,
}


#[derive(Debug, Serialize, Deserialize)]
struct CachedTrack {
    name: String,
    uri: String,
} 

#[derive(Debug, Serialize, Deserialize)]
struct CachedPlaylist {
    name: String,
    tracks: Vec<CachedTrack>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// Cache for playlists and access token.
/// Loaded from json upon start and saved to json upon exit.
#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    #[serde(skip)]
    file_path: String,
    playlists_by_name: std::collections::HashMap<String, CachedPlaylist>,
    token: spotify::Token,
}

impl Cache {

    fn new(file_path: &str) -> Self {
        Cache {
            file_path: file_path.to_string(),
            playlists_by_name: std::collections::HashMap::new(),
            token: spotify::Token::default(),
        }
    }

    /// Load from file, return default on errors with silent fallback to empty cache.
    /// 
    fn load_from_file(path: &str) -> Self {
        println!("Loading cache from file '{}'", path);
        
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("Failed to open cache file '{}': {}, starting with empty cache.", path, e);
                return Cache::new(path)
            }
        };

        let mut result: Cache = match serde_json::from_reader(file) {
            Ok(cache) => cache,
            Err(e) => {
                eprintln!("Failed to parse cache file '{}': {}, starting with empty cache.", path, e);
                Cache::new(path)
            }
        };

        result.file_path = path.to_string();
        result
    }

    pub fn load_and_retire_from_file(path: &str) -> Self {
        let mut result = Self::load_from_file(path);
        // Remove expired playlists
        result.retire_playlists();
        result
    }

    /// Save the cache, ignore errors. Fallback with warnings.
    pub fn save_to_file(&self) {
        let path = &self.file_path;
        println!("Saving cache to file '{}'", path);

        match std::fs::File::create(path) {
            Ok(file) => {
                if let Err(e) = serde_json::to_writer_pretty(file, self) {
                    eprintln!("Failed to write cache to file '{}': {}, cache will not be saved.", path, e);
                }
            }
            Err(e) => {
                eprintln!("Failed to create cache file '{}': {}, cache will not be saved.", path, e);
            }
        }
    }

    // Remove expired playlists from the cache. 
    fn retire_playlists(&mut self) {
        // retain only not expired playlists, i.e. remove expired ones
        self.playlists_by_name.retain(|_name, playlist| {
            let keep_it=chrono::Utc::now() < playlist.expires_at;
            let expires_in = playlist.expires_at - chrono::Utc::now();
            log::debug!("Playlist '{}' expires in {}, keep it: {}", playlist.name, expires_in, keep_it);
            keep_it
        }); 
    }

    pub fn cache_track(&mut self, playlist_name: &str, track_name: &str, track_uri: &str) {
        let playlist = self.playlists_by_name.entry(playlist_name.to_string()).or_insert_with(
            || CachedPlaylist {
                name: playlist_name.to_string(),
                tracks: vec![],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(26),
            }
        );

        log::info!("Caching track '{}' ({}) in playlist '{}'", track_name, track_uri, playlist_name);
        playlist.tracks.push(CachedTrack {
            name: track_name.to_string(),
            uri: track_uri.to_string(),
        });
    }
}

/// Load playlist from Spotify API if it's not already in cache or if it's expired.
/// If Spotify API call fails of getting song - ignore that and continue. Some songs may be unavailable due to various reasons (e.g. regional restrictions), but we don't want that to break the whole playlist loading.
async fn load_playlist_to_cache_if_dirty(cache: &mut Cache, client: &mut spotify::Client, playlist_name: &str) -> Result<(), spotify::SpotifyError> {

    if cache.playlists_by_name.contains_key(playlist_name) {
        log::debug!("Playlist '{}' found in cache, skipping Spotify API call.", playlist_name);
        return Ok(());
    }

    log::debug!("Playlist '{}' not found in cache, loading from Spotify API...", playlist_name);

    let playlist_stream = client.get_playlist_items(playlist_name).await?;

    futures::pin_mut!(playlist_stream);

    println!("Found playlist '{}', loading tracks...", playlist_name);
    while let Some(track_result) = playlist_stream.next().await {
        match track_result {
            Err(e) => {
                log::warn!("Error getting track: {}", e);
            },
            Ok((name, uri)) => {
                log::debug!("Have track: {} ({})", name, uri);
                cache.cache_track(playlist_name, &name, &uri);
            }
        }
    }

    Ok(())
}

fn select_random_tracks(tracks: &[CachedTrack], number_of_songs: usize) -> Vec<&CachedTrack> {
    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    tracks.choose_multiple(&mut rng, number_of_songs).collect()
}

// Get authorization parameters as defined in the registered Spotify application.
// Use secure way; in this case, environment variables.
fn load_auth_params_from_env() -> Result<spotify::AuthParams, Box<dyn std::error::Error>> {
    let client_id     = std::env::var("SPOTIFY_CLIENT_ID")?;
    let client_secret = std::env::var("SPOTIFY_CLIENT_SECRET")?;
    let default_redirect_uri = "http://127.0.0.1:58392".to_string();
    let redirect_uri  = std::env::var("SPOTIFY_REDIRECT_URI").unwrap_or(default_redirect_uri);
    Ok(spotify::AuthParams {
        client_id,
        client_secret,
        redirect_uri,
    })
}

/// Main function, extracted from main() to catch errors,
/// Updates the cache but does not provide persistence, so the caller (main()) can decide when to save it to file.
async fn run(playlist_name: &str, number_of_songs: usize, dry_run: bool, cache: &mut Cache) 
    -> Result<(), Box<dyn std::error::Error>> {

    // authenticate with Spotify API
    println!("Loading client...");
    let auth_params = load_auth_params_from_env()
        .map_err(|e| format!("Failed to load Spotify API credentials from environment variables: {}", e))?;

    let mut client = spotify::Client::new(cache.token.clone(), auth_params).await
        .map_err(|e| format!("Failed to authenticate with Spotify API: {}", e))?;
    println!("Done.");

    // enclosed in a block to ensure that receiving modified token from client on any error
    let result = {

        // load playlist to cache if it's not already there or expired
        load_playlist_to_cache_if_dirty(cache, &mut client, playlist_name).await
            .map_err(|e| format!("Failed to load playlist from Spotify API: {}", e))?;

        // get tracks from cache and print them
        let playlist = cache.playlists_by_name.get(playlist_name)
            .ok_or_else(|| format!("Playlist not found in cache: {}", playlist_name))?;
        
        let tracks = &playlist.tracks;
        for track in tracks {
            println!("Have track: {} ({}) (cached)", track.name, track.uri);
        }

        // draw random tracks from the playlist
        let random_tracks = select_random_tracks(tracks, number_of_songs);
        println!("Random tracks:");
        for track in random_tracks {
            if dry_run {
                println!("(dry-run) would add {} ({}) to queue", track.name, track.uri);
            } else {            
                println!("adding {} ({})", track.name, track.uri);
                client.add_to_queue(&track.uri).await
                    .map_err(|e| format!("Failed to add track to queue: {}", e))?;
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };

    cache.token = client.get_token(); // update token in cache before returning, so it can be saved to file in main()

    result
}

#[async_std::main]
async fn main() {    
    env_logger::init();

    println!("Starting...");

    // parse command line arguments
    let args = Cli::parse();

    // load cache from file, removing expired playlists
    let mut cache = Cache::load_and_retire_from_file("cache.json");

    // clean token if reauthentication is requested
    if args.reauthenticate {
        println!("Reauthentication requested, ignoring cached access token.");
        cache.token = spotify::Token::default();
    }

    // run the main logic 
    let run_result = run(
        &args.playlist_name, 
        args.number_of_songs, 
        args.dry_run, 
        &mut cache).await;

    // Save updated cache to file, even if run() returns an error.
    // This is single persistence point. 
    // If saving fails, the program will continue with a warning, but the cache will not be saved.
    cache.save_to_file();


    // handle errors, print them and exit with non-zero code
    if let Err(e) = run_result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
