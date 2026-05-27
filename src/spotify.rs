// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paweł Sklarow
//
// Spotify API integration layer for rustspo.
// Handles OAuth, token refresh, playlist traversal, track streaming, and queue updates.

use serde::{Serialize, Deserialize};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures::StreamExt;

use crate::http_listener; // Import the http_listener module to use its functions for handling HTTP requests during the authorization flow.

/// Internal module to hold API response structs and related types, for JSON serialization/deserialization. 
/// Part of Spotify official API, see https://developer.spotify.com/documentation/web-api/reference/.
mod api {
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    pub struct PaginatedResponse<T> {
        pub total: usize,
        pub items: Vec<T>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Playlist {
        pub name: String,
        pub id: String,
    }

    #[derive(Deserialize, Debug)]
    pub struct Track {
        pub name: String,
        pub uri: String,
    }

    #[derive(Deserialize, Debug)]
    pub struct SavedTrackObject {
        pub track: Track,
    }

    #[derive(Deserialize, Debug)]
    pub struct PlaylistTrackObject {
        pub item: Track,
    }

    #[derive(Deserialize, Debug)]
    #[serde(untagged)]
    pub enum TrackObject {
        SavedTrack(SavedTrackObject),
        PlaylistTrack(PlaylistTrackObject),
    }

    /// Response from Spotify API when requesting an access token using the Authorization Code flow. 
    /// See https://developer.spotify.com/documentation/web-api/tutorials/code-flow for details.
    #[derive(Deserialize, Debug)]
    pub struct SpotifyGetAccessTokenResponse {
        pub access_token: String,
        // pub token_type: String,
        pub expires_in: u64,
        pub refresh_token: Option<String>,
    }

}

/// Custom error type for Spotify-related errors.
#[derive(Debug, thiserror::Error)]
pub enum SpotifyError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("API error: {0}")]
    Api(String),
    #[error("Io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] url::ParseError),
}

/// Token to hold the access token, refresh token, and expiration time.
/// All fields obtained through the authorization flow.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Token {
    /// The access token used to authenticate requests to the Spotify API. 
    access_token: Option<String>,
    /// The expiration time of the access token. This is used to determine if the token needs to be refreshed.
    expires_at: chrono::DateTime<chrono::Utc>,
    /// The refresh token used to obtain a new access token when the current one expires. 
    refresh_token: Option<String>,
}
impl Token {
    fn is_expired(&self) -> bool {
        self.expires_at <= chrono::Utc::now()
    }
    fn set_expiration(&mut self, expires_in_seconds: u64) {
        self.expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in_seconds as i64);
    }
    
}
impl Default for Token {
    fn default() -> Self {
    Self {
            access_token: None,
            expires_at: chrono::Utc::now(),
            refresh_token: None,
        }
    }
}

/// Parameters needed for Spotify authorization. Obtained from the registered Spotify application 
/// and must be stored/provided in a safe manner (e.g. environment variables).
/// Part of spotify::Client.
pub struct AuthParams {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

/// Internal module to handle HTTP requests to the Spotify API, including error handling and response parsing.
mod http_transport {

use super::*;

    // Error conversion
    impl From<surf::Error> for SpotifyError {
        fn from(err: surf::Error) -> Self {
            SpotifyError::Http(err.to_string())
        }
    }    

    // Helper function to execute the HTTP request and handle errors. Used by both get and post methods.
    async fn do_http(method: http_types::Method, uri: &str, headers: &[(&str, &str)], body: Option<String>) -> Result<String, SpotifyError> {
        let uri = uri.parse()?;
        let mut request_builder = surf::RequestBuilder::new(method, uri);

        // add headers
        for (key, value) in headers {
            request_builder = request_builder.header(*key, *value);
        }

        // add body if present
        if let Some(body) = body {
            request_builder = request_builder.body(body);
        }

        // send the request and get the response
        let mut response = request_builder.await?;

        // handle error
        if !response.status().is_success() {
            let error_body = response.body_string().await.unwrap_or("[error reading body]".to_string());
            let message = match response.status() {
                surf::StatusCode::Unauthorized => {
                // 401 Unauthorized is a common error when the access token is invalid or expired. In this case, we can provide a more specific error message to help with debugging.
                        "HTTP 401 Unauthorized: This likely means that the access token is invalid or expired"
                }, 
                _ => "failed",
            };

            return Err(SpotifyError::Http(format!("Method: {} Error: {} Status {}: Body: {}", 
                method,
                message, 
                response.status(), 
                error_body))); 
        }

        let response_body = response.body_string().await?;
        Ok(response_body)
    }

    /// Sends an HTTP POST request
    pub async fn post(url: &str, headers: &[(&str, &str)], body: String) -> Result<String, SpotifyError> {
        do_http(http_types::Method::Post, url, headers, Some(body)).await
    }

    /// Sends an HTTP GET request
    pub async fn get(url: &str, headers: &[(&str, &str)]) -> Result<String, SpotifyError> {
        do_http(http_types::Method::Get, url, headers, None).await        
    }
}

/// Internal module to handle Spotify authorization flows, including opening the browser for user authorization,
/// handling the redirect with the authorization code, and exchanging the code for an access token.
/// 
/// Supports "Authorization Code Flow" authorization flow per https://developer.spotify.com/documentation/web-api/tutorials/code-flow.
/// Other flows (i.e. "Authorization Code with PKCE" and "Client Credentials Flow") are not supported.
/// 
/// # Commonly used names - Spotify Application parameters:
/// - client_id - the client ID of the registered Spotify application, used to identify the application during the authorization flow.
/// - client_secret - the client secret of the registered Spotify application, used to authenticate the application
/// - redirect_uri - the URI to which Spotify will redirect after the user authorizes the application.
/// 
mod auth {
use super::*;

/// Open the user's default web browser initiate the Spotify authorization.
/// After user authorizes acces, Spotify will redirect to local address with the authorization code.
/// First step of authorization based on Authorization Code Flow.
async fn initiate_authorize(client_id: &str, redirect_uri: &str) -> Result<(), SpotifyError> {
    let scope_string = "playlist-read-private user-library-read playlist-read-collaborative user-modify-playback-state";

    let url=url::Url::parse_with_params("https://accounts.spotify.com/authorize", 
    &[
        ("response_type", "code"),
        ("client_id", client_id),
        ("scope", scope_string),
        ("redirect_uri", redirect_uri),
    ]
    )?;

    log::info!("Opening browser for Spotify authorization...");
    open::that(url.as_str())
        .map_err(|e| SpotifyError::Api(format!("Failed to open browser for authorization: {}", e)))?;
    
    Ok(())
}

/// Get the authorization code from the redirect after user authorizes the application in the browser.
/// Second step of authorization based on Authorization Code Flow. 
/// Security invariant: redirect_uri host must be a literal loopback IP (127.0.0.1 or ::1). 
/// Hostnames are not allowed to prevent DNS rebinding attacks.
pub async fn get_auth_code(client_id: &str, redirect_uri: &str) -> Result<String, SpotifyError> {
    // open browser for authorization
    initiate_authorize(client_id, redirect_uri).await?;

    let uri: http::Uri = redirect_uri.parse()
        .map_err(|e| SpotifyError::Api(format!("Failed to parse redirect URI: {}", e)))?;
    let host = uri.host()       
        .ok_or_else(|| SpotifyError::Api("Redirect URI must have a host".to_string()))?;
    let port = uri.port_u16()
        .ok_or_else(|| SpotifyError::Api("Redirect URI must have a port".to_string()))?;
    // ip address always numeric due prevent DNS rebinding attacks
    let ip_addr = host.parse::<std::net::IpAddr>()
        .map_err(|e| SpotifyError::Api(format!("Failed to parse host in redirect URI: {}", e)))?;
    let path = uri.path();
    
    // wait for authorization response and get the full request URL
    let socket: std::net::SocketAddr = std::net::SocketAddr::new(ip_addr, port);

    let redirect_result = http_listener::accept_request(&socket, path, std::time::Duration::from_secs(60))?;


    // get required "code" parameter from the query parameters, return error if missing
    let code = redirect_result.query.get("code");

    match code {
        None => {
            Err(SpotifyError::Api("Authorization code not found".to_string()))
        },
        Some(code) => {
            Ok(code.to_string())
        }
    }
}

/// Get an access token using the Authorization Code flow.
/// Final step of authorization based on Authorization Code Flow,
/// requires the authorization code obtained in the previous step - get_auth_code().
pub async fn get_access_token_with_authorization_code(client_id: &str, client_secret: &str, code: &str, redirect_uri: &str) -> Result<api::SpotifyGetAccessTokenResponse, SpotifyError> {

    // generate post request body, use url::form_urlencoded to properly spaces, special characters, etc.
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .finish();

    // send post request 
    let auth_header_value = format!("Basic {}", BASE64.encode(format!("{}:{}", client_id, client_secret)));

    let body = http_transport::post(
        "https://accounts.spotify.com/api/token",
        &[("Content-Type", "application/x-www-form-urlencoded"),
            ("Authorization", &auth_header_value), ],
        form).await?;

    let token_response: api::SpotifyGetAccessTokenResponse = serde_json::from_str(&body)?;

    Ok(token_response)
}

/// Refresh an acces token per https://developer.spotify.com/documentation/web-api/tutorials/refreshing-tokens
pub async fn refresh_access_token(client_id: &str, client_secret: &str, refresh_token: &str) -> Result<api::SpotifyGetAccessTokenResponse, SpotifyError> {

    // generate post request body, use url::form_urlencoded to properly spaces, special characters, etc.
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh_token)
        .append_pair("client_id", client_id)
        .finish();

    // send post request 
    let auth_header_value = format!("Basic {}", BASE64.encode(format!("{}:{}", client_id, client_secret)));


    let body = http_transport::post(
        "https://accounts.spotify.com/api/token", 
        &[("Content-Type", "application/x-www-form-urlencoded"),
            ("Authorization", &auth_header_value), ],
        form).await?;
    let token_response: api::SpotifyGetAccessTokenResponse = serde_json::from_str(&body)?;

    Ok(token_response)
}

}

/// Spotify client
pub struct Client {
    token: Token,
    auth_params: AuthParams,
}

impl Client {

    /// Create a new Spotify client with the given token and authorization parameters. 
    /// The token can be empty (default) if no access token is obtained yet, 
    /// but the auth_params must be provided to allow the client to obtain an access token.
    pub async fn new(token: Token, auth_params: AuthParams) -> Result<Client, SpotifyError> {

        let result = Client{
            token,
            auth_params,
        };

        Ok(result)
    }

    /// Get the current token access token used to authenticate requests.
    /// Used e.g. for token persistence operations outside of the client.
    pub fn get_token(&self) -> Token {
        self.token.clone()
    }

    /// Get the access token used to authenticate HTTP requests to Spotify API ("Bearer"). 
    /// Refresh if expired. Authorize if no token obtained yet.
    /// Use package internally only.
    async fn get_access_token(&mut self) -> Result<String, SpotifyError> {

        log::info!("Getting access token...");

        let client_id = &self.auth_params.client_id;
        let client_secret = &self.auth_params.client_secret;
        let redirect_uri = &self.auth_params.redirect_uri;

        if self.token.access_token.is_none() {

                log::info!("No access token, starting authorization flow with auth code...");
                let auth_code = auth::get_auth_code(
                    client_id, 
                    redirect_uri).await?;

                log::debug!("Got auth code, exchanging for access token...");
                let token_get_reply = auth::get_access_token_with_authorization_code(
                    client_id, 
                    client_secret, 
                    &auth_code, 
                    redirect_uri).await?;

                log::debug!("Got access token, expires in {} seconds", token_get_reply.expires_in);

                self.token.set_expiration(token_get_reply.expires_in);
                self.token.refresh_token = token_get_reply.refresh_token;
                self.token.access_token = Some(token_get_reply.access_token.clone());
                Ok(token_get_reply.access_token)

        } else {

            log::debug!("Have access token, checking if expired...");

            // have access token, check if expired
            if self.token.is_expired() {

                log::info!("Access token is expired, refreshing...");

                // expired, do refresh per
                // https://developer.spotify.com/documentation/web-api/tutorials/refreshing-tokens

                let refresh_token = self.token.refresh_token.as_ref().ok_or(
                    // if the token is expired and we don't have a refresh token, we cannot refresh the token, so we need to re-authorize
                    SpotifyError::Api("Access token is expired and no refresh token available".to_string())
                )?;

                log::debug!("Have refresh token, refreshing access token...");
                let token_refresh_reply = auth::refresh_access_token(
                    client_id,
                    client_secret,
                    refresh_token).await?;

                log::debug!("Got refreshed access token");

                self.token.set_expiration(token_refresh_reply.expires_in);
                
                if token_refresh_reply.refresh_token.is_some() {
                    // update refresh token if a new one is provided in the refresh response
                    self.token.refresh_token = token_refresh_reply.refresh_token;
                }
                
                self.token.access_token = Some(token_refresh_reply.access_token);

            }
            self.token.access_token.clone().ok_or_else(
                || SpotifyError::Api("Access token is expired and no refresh token available".to_string())
            )
        }
    }

    /// Generic function to get items from a paginated Spotify API endpoint as a stream.
    /// The caller provides a URL template with {limit} and {offset} placeholders.
    /// The function handles pagination and yields items one by one until all items are retrieved.
    fn items_stream<T: for <'de> Deserialize<'de>>(
        &mut self,
        url_template: String,
        access_token: String,
    ) -> impl futures::Stream<Item = Result<T, SpotifyError>> {
        use async_stream::stream;
        
        stream! {
            let mut offset = 0;
            let offset_increment = 50;

            loop {
                let url = url_template
                    .replace("{limit}", &offset_increment.to_string())
                    .replace("{offset}", &offset.to_string());
                
                log::debug!("Fetching items with URL: {}", url);

                let auth_header_value = format!("Bearer {}", access_token);
                let body = http_transport::get(&url, &[("Authorization", &auth_header_value)]).await?;

                let response: api::PaginatedResponse<T> = 
                    serde_json::from_str(&body)
                        .map_err(|e| SpotifyError::Api(format!("Failed to parse JSON response: {}, body: {}", e, body)))?;                
                // TODO better error handling for JSON parsing, include the body in the error message for easier debugging, e.g. by using helper function that wraps serde_json::from_str and adds the body to the error message

                log::debug!("New piece {}-{}/{}... ", offset, offset + response.items.len(), response.total);

                for item in response.items {
                    yield Ok(item);
                }

                offset += offset_increment;
                if offset >= response.total {
                    break;
                }
            }
        }
    }

    /// Get all items from a playlist as a stream. 
    /// The playlist is identified by its name.
    /// If the playlist name is "All Songs", return all songs in the user's library.
    pub async fn get_playlist_items(&mut self, playlist_name: &str) -> Result<impl futures::Stream<Item = Result<(String, String), SpotifyError>>, SpotifyError> {

        let url_template = 
            if playlist_name == "All Songs" {
                // "All Songs" is a special playlist that represents all songs in the user's library. We can get it using the /me/tracks endpoint.
                "https://api.spotify.com/v1/me/tracks?limit={limit}&offset={offset}".to_string()

            } else {
                let playlist_id = self.find_playlist_id_by_name(playlist_name).await?;
                let playlist_id = playlist_id.ok_or(
                    SpotifyError::Api(format!("Playlist '{}' not found in user's library", playlist_name))
                )?;
                log::info!("Found playlist '{}' with id '{}'", playlist_name, playlist_id);
                
                format!(
                    "https://api.spotify.com/v1/playlists/{}/items?limit={{limit}}&offset={{offset}}", playlist_id
                )
            };

        let access_token = self.get_access_token().await?;
        let stream = self.items_stream::<api::TrackObject>(url_template, access_token);
        let result = stream.map(|track_result| {
            // per-item transformation - got an api::TrackObject
            track_result.map(|track_object| {
                // convert to (name, uri) tuple.
                // thats the same object type, but under different names ("track" in saved tracks, "item" in playlist tracks), so we need to handle both cases.
                let track = match track_object {
                        api::TrackObject::SavedTrack(obj) => obj.track,
                        api::TrackObject::PlaylistTrack(obj) => obj.item,
                    };
                (track.name, track.uri)
            })
        });

        Ok(result)
    }

    /// Find a playlist ID by its name. 
    /// Returns None if no playlist with the given name is found in the user's library, or error 
    /// (e.g. network error, JSON parsing error, etc.)
    async fn find_playlist_id_by_name(&mut self, name: &str) -> Result<Option<String>, SpotifyError> {
        use futures::{pin_mut, StreamExt};
        let url_template = "https://api.spotify.com/v1/me/playlists?limit={limit}&offset={offset}".to_string(); 

        let access_token = self.get_access_token().await?;
        let stream = self.items_stream::<api::Playlist>(url_template, access_token);
        pin_mut!(stream);
        
        while let Some(playlist_fetch_result) = stream.next().await {
            // extract playlist, populate potential error up ("?")
            let playlist = playlist_fetch_result?;

            log::debug!("Checking playlist: '{}'", playlist.name);

            if playlist.name == name {

                log::debug!("Got playlist: {:?}", playlist);

                return Ok(Some(playlist.id.to_string()));
            }
        }

        Ok(None)
    }

    /// Add a track to the playback queue by its Spotify URI.
    pub async fn add_to_queue(&mut self, track_uri: &str) -> Result<(), SpotifyError> {
        let url = format!("https://api.spotify.com/v1/me/player/queue?uri={}", urlencoding::encode(track_uri));
        let access_token = self.get_access_token().await?;
        let auth_header_value = format!("Bearer {}", access_token);
        http_transport::post(&url, &[("Authorization", &auth_header_value)], String::new()).await?;
        Ok(())
    }
}

