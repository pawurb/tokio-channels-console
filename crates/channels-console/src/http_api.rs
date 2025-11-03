use crate::{get_channel_logs, get_serializable_stats};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use std::fmt::Display;
use tiny_http::{Header, Request, Response, Server};

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
        "/metrics" => {
            let stats = get_serializable_stats();
            respond_json(request, &stats);
        }
        _ => {
            if let Some(encoded_id) = path.strip_prefix("/logs/") {
                match decode_channel_id(encoded_id) {
                    Ok(channel_id) => match get_channel_logs(&channel_id) {
                        Some(logs) => respond_json(request, &logs),
                        None => respond_error(request, 404, "Channel not found"),
                    },
                    Err(msg) => respond_error(request, 400, msg),
                }
            } else {
                respond_error(request, 404, "Not found");
            }
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

fn decode_channel_id(encoded: &str) -> Result<String, &'static str> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Invalid base64 encoding")?;
    String::from_utf8(bytes).map_err(|_| "Invalid channel ID encoding")
}
