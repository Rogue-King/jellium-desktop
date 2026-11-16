//! Connection failures safe to display without exposing request URLs or credentials.

use cef::sys::cef_errorcode_t as Code;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConnectionError {
    #[error(
        "The server name could not be resolved. Check its spelling and your DNS or VPN connection."
    )]
    Dns,
    #[error(
        "The server refused the connection. Check that Jellyfin is running and the port is correct."
    )]
    Refused,
    #[error("The connection timed out. Check the server, network, and VPN, then retry.")]
    Timeout,
    #[error(
        "A secure connection could not be established (Chromium error {0}). Check the server certificate, HTTPS configuration, and system clock."
    )]
    Tls(i32),
    #[error(
        "The address is invalid or uses an unsupported scheme. Enter an HTTP or HTTPS Jellyfin address."
    )]
    InvalidUrl,
    #[error(
        "The server returned HTTP {0}. Check the address, reverse proxy, and access permissions."
    )]
    Http(i32),
    #[error(
        "The address did not return valid Jellyfin server information. Check the server address and any reverse-proxy path."
    )]
    InvalidServer,
    #[error("The request failed (Chromium error {0}). Check your network and server, then retry.")]
    Network(i32),
    #[error("The connection request could not be started. Retry, or restart the application.")]
    Unavailable,
}

impl ConnectionError {
    pub(crate) fn from_cef(code: i32) -> Self {
        if code == Code::ERR_NAME_NOT_RESOLVED as i32 {
            Self::Dns
        } else if code == Code::ERR_CONNECTION_REFUSED as i32 {
            Self::Refused
        } else if code == Code::ERR_TIMED_OUT as i32
            || code == Code::ERR_CONNECTION_TIMED_OUT as i32
        {
            Self::Timeout
        } else if code == Code::ERR_INVALID_URL as i32
            || code == Code::ERR_UNKNOWN_URL_SCHEME as i32
        {
            Self::InvalidUrl
        } else if (code <= Code::ERR_CERT_COMMON_NAME_INVALID as i32
            && code > Code::ERR_CERT_END as i32)
            || code == Code::ERR_SSL_PROTOCOL_ERROR as i32
            || code == Code::ERR_SSL_VERSION_OR_CIPHER_MISMATCH as i32
            || code == Code::ERR_SSL_CLIENT_AUTH_CERT_NEEDED as i32
        {
            Self::Tls(code)
        } else {
            Self::Network(code)
        }
    }
}

pub(crate) fn validate_server_response(status: i32, body: &[u8]) -> Result<(), ConnectionError> {
    if status != 200 {
        Err(ConnectionError::Http(status))
    } else if !jfn_jellyfin::is_valid_public_info(body) {
        Err(ConnectionError::InvalidServer)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_network_certificate_and_address_failures() {
        for (code, expected) in [
            (Code::ERR_NAME_NOT_RESOLVED, ConnectionError::Dns),
            (Code::ERR_CONNECTION_REFUSED, ConnectionError::Refused),
            (Code::ERR_TIMED_OUT, ConnectionError::Timeout),
            (Code::ERR_INVALID_URL, ConnectionError::InvalidUrl),
            (
                Code::ERR_CERT_AUTHORITY_INVALID,
                ConnectionError::Tls(Code::ERR_CERT_AUTHORITY_INVALID as i32),
            ),
            (
                Code::ERR_SSL_PROTOCOL_ERROR,
                ConnectionError::Tls(Code::ERR_SSL_PROTOCOL_ERROR as i32),
            ),
        ] {
            assert_eq!(ConnectionError::from_cef(code as i32), expected);
        }
    }

    #[test]
    fn distinguishes_http_errors_from_non_jellyfin_responses() {
        assert_eq!(
            validate_server_response(403, b"forbidden"),
            Err(ConnectionError::Http(403))
        );
        assert_eq!(
            validate_server_response(200, b"<html>proxy login</html>"),
            Err(ConnectionError::InvalidServer)
        );
        assert_eq!(validate_server_response(200, br#"{"Id":"server"}"#), Ok(()));
    }
}
