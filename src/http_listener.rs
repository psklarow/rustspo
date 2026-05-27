
// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paweł Sklarow
//
// Local HTTP callback listener used during Spotify OAuth.
// Waits for a single request and extracts the request path and query parameters.

use tiny_http::{Server, Response};
use std::collections::HashMap;
use url::{form_urlencoded};
use std::io::Error as IoError;
use std::io::ErrorKind as IoErrorKind;
use http::uri::Uri;


pub struct ListenResult {

    pub path: String,

    /// Query parameters parsed from the request URL, stored as a HashMap where the key is the parameter name and the value is the parameter value.
    pub query: HashMap<String, String>,

}

/// Parse request in string form into ListenResult
fn parse_request(request_str:  &str) -> Result<ListenResult, http::Error> {
    let uri: Uri = request_str.parse()?;
    let path = uri.path();
    let possible_query = uri.query();

    // Parse the query parameters into a HashMap. If there are no query parameters, return an empty HashMap.
    let query_map = possible_query.map(|query| {
        form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect::<HashMap<String, String>>()
    }).unwrap_or_else(|| {
        HashMap::new()
    });

    Ok(ListenResult {
        path: path.to_string(),
        query: query_map,
    })
}

/// Wait for an HTTP request on the specified address.
pub fn accept_request(addr: &std::net::SocketAddr, required_path: &str, timeout: std::time::Duration) -> Result<ListenResult, IoError> {
    log::debug!("accept_request({});", addr);

    // Start the HTTP server
    let server = Server::http(addr);

    // convert to regular error with message if server fails to start
    let server = server.map_err(|e| IoError::other(format!("Failed to start HTTP server: {}", e))
    )?;

    let end_time = std::time::Instant::now() + timeout;    

    loop {

        // Wait for a single request
        let remaining_time = end_time.saturating_duration_since(std::time::Instant::now());
        match server.recv_timeout(remaining_time)? {
            Some(request) => {

                log::debug!("Received request: '{}'", request.url());
                let request_str = request.url().to_string();
                let result = parse_request(&request_str)
                    .map_err(|e| IoError::new(IoErrorKind::InvalidData, format!("Failed to parse request: {}", e)))?;

                if result.path == required_path {
                    // Send a simple response so the browser doesn't hang
                    let body = "You may close this window.";
                    let _ = request.respond(Response::from_string(body));
                    return Ok(result);
                } else {
                    // Send an 404 response for requests that don't match the required path
                    log::warn!("Received request with path '{}', but expected '{}'. 404ed.", result.path, required_path);
                    let body = "404 Not Found";
                    let _ = request.respond(Response::from_string(body).with_status_code(404));
                }

                if std::time::Instant::now() >= end_time {
                    return Err(IoError::new(IoErrorKind::TimedOut, "Timed out waiting for HTTP request"));
                }

            },
            None => {
                return Err(IoError::new(IoErrorKind::TimedOut, "Timed out waiting for HTTP request"));
            }
        }
    }
}


#[cfg(test)]
use async_std::task::block_on;

// Tests
#[async_std::test]
async fn wait_for_http_request_test() {

    // Send test data in delayed thread to avoid blocking the main thread which is waiting for the HTTP request. 
    //This simulates an actual client making a request to the server.
    std::thread::spawn(|| block_on(async {
        println!("Start sending thread...");
        // delay to ensure the server is up and waiting for the request before we send it
        std::thread::sleep(std::time::Duration::from_millis(500));

        println!("Sending test HTTP request...");

        // send request using surf
        let url = format!("http://localhost:56780/test?param1=value1&param2=20000");
        let _response = surf::get(&url).await;
        println!("Test HTTP request sent.");
    }));

    // wait for HTTP request and check that the received URL is correct
    let socket: std::net::SocketAddr = "127.0.0.1:56780".parse()
        .map_err(|e| panic!("Failed to parse socket address: {}", e)).unwrap();
    let result = accept_request(&socket, "/test", std::time::Duration::from_secs(5));

    assert!(result.is_ok(), "accept_request failed: {}", result.err().unwrap());    
    let result = result.unwrap();

    assert_eq!(result.path, "/test");
    assert_eq!(result.query.get("param1").unwrap(), "value1");
    assert_eq!(result.query.get("param2").unwrap(), "20000");
}

#[test]
fn parse_query_parameters_test() {
    let request = "/callback?code=abc123&state=xyz789";
    let params = parse_request(request).unwrap();
    assert_eq!(params.query.get("code").unwrap(), "abc123");
    assert_eq!(params.query.get("state").unwrap(), "xyz789");
    assert_eq!(params.path, "/callback");
}
