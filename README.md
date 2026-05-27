# rustspo

rustspo is a Rust CLI application that fills the Spotify playback queue with a random selection of tracks from one of the user's playlists.

## Features

- Spotify Authorization Code flow with refresh-token handling
- Local loopback HTTP listener for the OAuth redirect callback
- Playlist caching to reduce repeated API calls during normal use
- Random track selection from any playlist in the user's library
- Special `All Songs` mode for the saved tracks library
- `--dry-run` mode for safe testing without modifying the playback queue

## Technologies, Idioms, and Language Features

### Technologies

- Rust 2024 for the full application, including CLI workflow, OAuth handling, caching, and API integration
- Spotify Web API with Authorization Code flow and refresh-token handling
- `clap` derive macros for command-line argument parsing
- `surf` as the HTTP client for Spotify API calls and token exchange
- `tiny_http` for a local loopback callback server used during OAuth login
- `serde` and `serde_json` for JSON deserialization of API payloads and persistence of the local cache
- `chrono` for token expiry and cache invalidation timestamps
- `async-std`, `futures`, and `async-stream` for async control flow and paginated streaming of Spotify data
- `log` and `env_logger` for runtime logging
- `rand` for unbiased random track selection
- `url`, `http`, `base64`, and `open` for URI construction/parsing, auth header generation, and browser-based login initiation

### Rust idioms and engineering practices

- Separation of concerns across modules: CLI/workflow in `main.rs`, Spotify integration in `spotify.rs`, and OAuth callback listening in `http_listener.rs`
- Strongly typed domain modeling with dedicated structs and enums for playlists, tracks, token state, cache entries, and API responses
- `Result`-based error propagation with a custom error type built using `thiserror`
- Explicit error conversion via `From` implementations instead of loosely typed string-only handling
- Ownership and borrowing patterns for shared mutable state such as the cache and authenticated client
- Iterator and collection-oriented style using `HashMap`, `retain`, `entry(...).or_insert_with(...)`, and random sampling with `choose_multiple`
- Stream-based processing of paginated API results rather than loading all responses through one oversized control path
- Best-effort failure handling where individual track-load failures do not abort the entire playlist load
- Security-conscious OAuth handling, including strict validation of loopback redirect hosts and local callback path matching
- Local persistence with cache retirement/expiry logic instead of re-fetching everything on every run

### Rust language features touched in this project

- `async` / `await`
- Modules and visibility boundaries
- Structs, enums, and impl blocks
- Generic functions and typed stream construction
- Pattern matching with `match`
- Trait derives such as `Parser`, `Serialize`, `Deserialize`, `Debug`, and `Clone`
- Manual `Default` implementation for token state
- Lifetimes through borrowed data in function signatures and returned references
- Error handling with `?`, custom enums, and boxed trait objects where appropriate
- Attribute macros including `#[derive(...)]`, `#[async_std::main]`, and test attributes

## How It Works

1. The application reads Spotify API credentials from environment variables.
2. It authenticates the user through Spotify's OAuth flow.
3. It loads playlist contents from cache or from the Spotify Web API.
4. It selects a random subset of tracks.
5. It adds those tracks to the current Spotify playback queue.

## Requirements

- Rust toolchain with Cargo
- A Spotify Premium account for queue modification features
- A Spotify application registered in the Spotify Developer Dashboard

## Configuration

Set these environment variables before running the application:

- `SPOTIFY_CLIENT_ID`
- `SPOTIFY_CLIENT_SECRET`
- `SPOTIFY_REDIRECT_URI` (optional)

If `SPOTIFY_REDIRECT_URI` is not set, the application defaults to `http://127.0.0.1:58392`.

The redirect URI configured in Spotify Developer Dashboard must match the value used by the application.

## Usage

Run the application with a playlist name:

```bash
cargo run -- "My Playlist"
```

Select a custom number of tracks:

```bash
cargo run -- "My Playlist" -n 30
```

Use the full saved-tracks library:

```bash
cargo run -- "All Songs"
```

Preview the selected tracks without modifying the queue:

```bash
cargo run -- "My Playlist" --dry-run
```

Force a fresh Spotify login:

```bash
cargo run -- "My Playlist" --reauthenticate
```

## Development

Run the test suite:

```bash
cargo test
```

Build a release binary:

```bash
cargo build --release
```

## Project Restrictions

The following behavior is intentional project policy and should be preserved unless requirements change:

- OAuth callback host validation is intentionally strict. The redirect host must be a literal IP address, not a hostname, to reduce DNS rebinding and local DNS poisoning risk.
- Playlist loading is intentionally best-effort. Per-track fetch failures do not abort the whole playlist load because partial results are acceptable for this CLI.
- Existing authentication logs are intentionally non-sensitive. Flow-state logging is acceptable as long as auth codes, access tokens, refresh tokens, and raw token payloads are never logged.
- Empty playlists are intentionally not cached. A cached playlist must contain tracks to be useful for random queue population.

## Repository Layout

- `src/main.rs` contains the CLI, cache handling, and application workflow
- `src/spotify.rs` contains Spotify API authentication and API access logic
- `src/http_listener.rs` contains the local HTTP callback listener used during OAuth

## TODO

- Add unit tests for cache lifecycle behavior, including playlist expiry and token persistence edge cases.
- Add tests for random track selection covering empty, undersized, and exact-size playlists.
- Add isolated tests for Spotify response parsing and pagination behavior using mocked API payloads.
- Add authorization-flow tests around redirect URI validation and callback query parsing.
- Add failure-path tests for partial playlist loading to verify that per-track errors remain non-fatal.

## License

This project is licensed under the MIT License. See the `LICENSE` file for details.