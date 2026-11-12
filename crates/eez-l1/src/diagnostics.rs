//! Log-safe diagnostics for operator-configured HTTP endpoints.
//!
//! Remote bodies and arbitrary error sources may echo credentials, including
//! path/query tokens without a URL. Only typed, nonsecret metadata is emitted.

use std::{error::Error, fmt::Write as _};

use alloy_transport::{RpcError, TransportError, TransportErrorKind};

/// Identify an endpoint by host only, as with `BuilderRelays::label`.
/// Never use the original URL as a fallback for malformed/opaque URLs.
pub fn endpoint_host(url: &url::Url) -> &str {
    url.host_str().unwrap_or("<unknown>")
}

/// Describe an HTTP failure without formatting its URL or arbitrary sources.
/// Nested errors contribute only typed metadata; the bounded walk also handles
/// custom sources whose chain does not terminate.
pub fn http_error(error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "HTTP request timed out"
    } else if error.is_connect() {
        "HTTP connection failed"
    } else if error.is_redirect() {
        "HTTP redirect failed"
    } else if error.is_decode() {
        "HTTP response decode failed"
    } else if error.is_body() {
        "HTTP body failed"
    } else if error.is_builder() {
        "HTTP request configuration failed"
    } else {
        "HTTP request failed"
    };
    let mut message = kind.to_owned();
    if let Some(status) = error.status() {
        write!(message, " (status {})", status.as_u16()).expect("writing to String cannot fail");
    }
    let mut source = error.source();
    for _ in 0..16 {
        let Some(cause) = source else { break };
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            write!(message, ": {:?}", io.kind()).expect("writing to String cannot fail");
            break;
        }
        source = cause.source();
    }
    message
}

/// Describe an RPC failure without remote messages, response bodies or custom
/// error text. The caller supplies the operation being attempted.
pub fn rpc_error(error: &TransportError) -> String {
    match error {
        RpcError::ErrorResp(response) => {
            format!("RPC error {}", response.code)
        }
        RpcError::NullResp => "RPC returned a null response".into(),
        RpcError::UnsupportedFeature(_) => "RPC feature unsupported".into(),
        RpcError::LocalUsageError(_) => "RPC local usage error".into(),
        RpcError::SerError(_) => "RPC request serialization failed".into(),
        RpcError::DeserError { .. } => "RPC response decode failed".into(),
        RpcError::Transport(kind) => match kind {
            TransportErrorKind::HttpError(error)
            | TransportErrorKind::HttpErrorWithRetryAfter { error, .. } => {
                format!("HTTP request failed (status {})", error.status)
            }
            TransportErrorKind::BackendGone => "RPC backend stopped".into(),
            TransportErrorKind::PubsubUnavailable => "RPC subscriptions unavailable".into(),
            TransportErrorKind::MissingBatchResponse(_) => "RPC batch response missing".into(),
            _ => {
                let mut source: Option<&(dyn Error + 'static)> = Some(kind);
                for _ in 0..16 {
                    let Some(cause) = source else { break };
                    if let Some(http) = cause.downcast_ref::<reqwest::Error>() {
                        return http_error(http);
                    }
                    source = if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                        io.get_ref().map(|inner| inner as &(dyn Error + 'static))
                    } else {
                        cause.source()
                    };
                }
                "RPC transport failed".into()
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "sentinel-credential";

    #[tokio::test]
    async fn nested_transport_errors_never_format_untrusted_sources() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{SECRET}:{SECRET}@{addr}/{SECRET}?key={SECRET}");
        let error = reqwest::Client::new().get(url).send().await.unwrap_err();
        assert!(format!("{error:?}").contains(SECRET));
        let safe = http_error(&error);
        assert!(safe.contains("connection failed"), "{safe}");
        assert!(!safe.contains(SECRET));
        let error = TransportErrorKind::custom(std::io::Error::other(error));
        let safe = rpc_error(&error);
        assert!(safe.contains("connection failed"), "{safe}");
        assert!(!safe.contains(SECRET));
        let arbitrary = TransportErrorKind::custom(std::io::Error::other(SECRET));
        assert_eq!(rpc_error(&arbitrary), "RPC transport failed");
    }

    #[test]
    fn rpc_diagnostics_keep_codes_and_range_policy_without_remote_text() {
        let error: TransportError = RpcError::ErrorResp(serde_json::from_value(
            serde_json::json!({"code":-32005,"message":format!("too many results {SECRET}"),"data":SECRET})
        ).unwrap());
        let safe = crate::scan::log_query_error(&error);
        assert!(safe.contains("-32005"));
        assert!(!safe.contains(SECRET));
        assert!(crate::scan::is_range_too_wide(&crate::L1Error::Provider(
            safe
        )));
        let error = TransportErrorKind::http_error(503, SECRET.into());
        let safe = rpc_error(&error);
        assert!(safe.contains("503"));
        assert!(!safe.contains(SECRET));
        let bad_json = serde_json::from_str::<u64>(&format!("\"{SECRET}\"")).unwrap_err();
        let safe = rpc_error(&RpcError::DeserError {
            err: bad_json,
            text: SECRET.into(),
        });
        assert_eq!(safe, "RPC response decode failed");
    }
}
