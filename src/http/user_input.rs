use std::convert::TryFrom;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use http::header::HeaderValue;
use http::uri::Uri;
use http::{HeaderMap, Method};
use hyper::body::Bytes;
use rustls::ClientConfig;
use tokio_rustls::TlsConnector;

use super::no_server_verifier::NoServerVerifier;
use super::BenchType;

#[derive(Clone)]
pub(crate) enum Scheme {
  Http,
  Https(TlsConnector),
}

impl Scheme {
  fn default_port(&self) -> u16 {
    match self {
      Self::Http => 80,
      Self::Https(_) => 443,
    }
  }
}

#[derive(Clone)]
pub(crate) struct UserInput {
  pub(crate) addr: SocketAddr,
  pub(crate) scheme: Scheme,
  pub(crate) host: String,
  pub(crate) host_header: HeaderValue,
  pub(crate) uri: Uri,
  pub(crate) method: Method,
  pub(crate) headers: HeaderMap,
  pub(crate) body: Bytes,
}

impl UserInput {
  pub(crate) async fn new(
    protocol: BenchType,
    string: String,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
  ) -> Result<Self> {
    let uri = Uri::try_from(string)?;
    let scheme = uri
      .scheme()
      .ok_or_else(|| anyhow!("scheme is not present on uri"))?
      .as_str();
    let scheme = match scheme {
      "http" => Scheme::Http,
      "https" => {
        let mut client_config = ClientConfig::builder()
          .dangerous()
          .with_custom_certificate_verifier(Arc::new(NoServerVerifier::new()))
          .with_no_client_auth();

        client_config.alpn_protocols = match protocol {
          BenchType::HTTP1 => vec![b"http/1.1".to_vec(), b"http/1.0".to_vec()],
          BenchType::HTTP2 => vec![b"h2".to_vec()],
        };

        let connector = TlsConnector::from(Arc::new(client_config));
        Scheme::Https(connector)
      }
      _ => return Err(anyhow::Error::msg("invalid scheme")),
    };
    let authority = uri
      .authority()
      .ok_or_else(|| anyhow!("host not present on uri"))?;
    let host = authority.host().to_owned();
    let port = authority
      .port_u16()
      .unwrap_or_else(|| scheme.default_port());
    let host_header = HeaderValue::from_str(&host)?;

    // Prefer ipv4.
    let addr_iter = (host.as_str(), port).to_socket_addrs()?;
    let mut last_addr = None;
    for addr in addr_iter {
      last_addr = Some(addr);
      if addr.is_ipv4() {
        break;
      }
    }
    let addr = last_addr.ok_or_else(|| anyhow!("hostname lookup failed"))?;

    Ok(Self {
      addr,
      scheme,
      host,
      host_header,
      uri,
      method,
      headers,
      body,
    })
  }
}
