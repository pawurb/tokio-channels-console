use crate::{get_channel_logs, get_channels_json, get_stream_logs, get_streams_json};
use regex::Regex;
use serde::Serialize;
use std::fmt::Display;
use std::sync::LazyLock;
use tiny_http::{Header, Request, Response, Server};

static RE_CHANNEL_LOGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/channels/(\d+)/logs$").unwrap());
static RE_STREAM_LOGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/streams/(\d+)/logs$").unwrap());

pub(crate) fn start_metrics_server(addr: &str) {
    let server = match Server::http(addr) {
        Ok(s) => s,
        Err(e) => {
            panic!("Failed to bind metrics server to {}: {}. Customize the port using the CHANNELS_CONSOLE_METRICS_PORT environment variable.", addr, e);
        }
    };

    println!("Channel metrics server listening on http://{}", addr);

    for request in server.incoming_requests() {
        handle_request(request);
    }
}

fn handle_request(request: Request) {
    let path = request.url().split('?').next().unwrap_or("/");

    match path {
        "/channels" => {
            let channels = get_channels_json();
            respond_json(request, &channels);
        }
        "/streams" => {
            let streams = get_streams_json();
            respond_json(request, &streams);
        }
        _ => {
            // Handle /channels/<id>/logs
            if let Some(caps) = RE_CHANNEL_LOGS.captures(path) {
                return match get_channel_logs(&caps[1]) {
                    Some(logs) => respond_json(request, &logs),
                    None => respond_error(request, 404, "Channel not found"),
                };
            }

            // Handle /streams/<id>/logs
            if let Some(caps) = RE_STREAM_LOGS.captures(path) {
                return match get_stream_logs(&caps[1]) {
                    Some(logs) => respond_json(request, &logs),
                    None => respond_error(request, 404, "Stream not found"),
                };
            }

            respond_error(request, 404, "Not found");
        }
    }
}

fn respond_json<T: Serialize>(request: Request, value: &T) {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::from_data(body);
            response.add_header(
                Header::from_bytes(b"Content-Type".as_slice(), b"application/json".as_slice())
                    .unwrap(),
            );
            let _ = request.respond(response);
        }
        Err(e) => respond_internal_error(request, e),
    }
}

fn respond_error(request: Request, code: u16, msg: &str) {
    let _ = request.respond(Response::from_string(msg).with_status_code(code));
}

fn respond_internal_error(request: Request, e: impl Display) {
    eprintln!("Internal server error: {}", e);
    let _ = request.respond(
        Response::from_string(format!("Internal server error: {}", e)).with_status_code(500),
    );
}
