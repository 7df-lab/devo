//! Minimal localhost HTTP CONNECT proxy for sandboxed processes with restricted
//! network access.
//!
//! Binds `127.0.0.1:0`, publishes the port via [`devo_sandbox::set_sandbox_proxy_ports`],
//! and forwards outbound TCP through HTTP CONNECT.

use std::sync::Arc;

use anyhow::Context;
use devo_sandbox::{ManagedNetworkSandboxContext, set_sandbox_proxy_ports};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// Running sandbox network proxy and its loopback listener port.
pub struct SandboxNetworkProxyHandle {
    http_port: u16,
    cancel: CancellationToken,
    server_task: tokio::task::JoinHandle<()>,
}

impl SandboxNetworkProxyHandle {
    pub fn http_port(&self) -> u16 {
        self.http_port
    }

    pub fn socks_port(&self) -> Option<u16> {
        None
    }

    pub fn managed_context(&self) -> ManagedNetworkSandboxContext {
        devo_sandbox::managed_network_context_from_ports([self.http_port])
    }
}

impl Drop for SandboxNetworkProxyHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.server_task.abort();
    }
}

/// Starts a localhost HTTP CONNECT proxy and publishes its port for
/// in-process managed-network lookups (thread-safe; no process-wide
/// `env::set_var` from the Tokio worker).
pub async fn start_sandbox_network_proxy() -> anyhow::Result<SandboxNetworkProxyHandle> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind sandbox network proxy")?;
    let http_port = listener
        .local_addr()
        .context("failed to read sandbox network proxy address")?
        .port();
    set_sandbox_proxy_ports(&[http_port]);

    let cancel = CancellationToken::new();
    let server_cancel = cancel.clone();
    let server_task = tokio::spawn(async move {
        run_proxy_server(listener, server_cancel).await;
    });

    tracing::info!(http_port, "sandbox network proxy listening");

    Ok(SandboxNetworkProxyHandle {
        http_port,
        cancel,
        server_task,
    })
}

async fn run_proxy_server(listener: TcpListener, cancel: CancellationToken) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        tokio::spawn(handle_client(stream));
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "sandbox network proxy accept failed");
                        break;
                    }
                }
            }
        }
    }
}

async fn handle_client(mut client: TcpStream) {
    let mut header_buf = Vec::with_capacity(4096);
    if read_http_headers(&mut client, &mut header_buf)
        .await
        .is_err()
    {
        return;
    }

    let request_line = header_buf
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or(&header_buf);
    let request_line = String::from_utf8_lossy(request_line).trim_end().to_string();

    if let Some(target) = parse_connect_target(&request_line) {
        proxy_connect(client, &header_buf, target).await;
        return;
    }

    if let Some((host, port, path)) = parse_get_target(&request_line) {
        proxy_get(client, &header_buf, host, port, path).await;
    }
}

async fn read_http_headers(client: &mut TcpStream, header_buf: &mut Vec<u8>) -> anyhow::Result<()> {
    let mut chunk = [0u8; 1024];
    loop {
        let read = client
            .read(&mut chunk)
            .await
            .context("failed to read proxy request")?;
        if read == 0 {
            anyhow::bail!("proxy client closed before sending headers");
        }
        header_buf.extend_from_slice(&chunk[..read]);
        if header_buf.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(());
        }
        if header_buf.len() > 16 * 1024 {
            anyhow::bail!("proxy request headers too large");
        }
    }
}

fn header_body_offset(headers: &[u8]) -> usize {
    headers
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .unwrap_or(headers.len())
}

fn parse_connect_target(request_line: &str) -> Option<(String, u16)> {
    let mut parts = request_line.split_whitespace();
    if parts.next()? != "CONNECT" {
        return None;
    }
    parse_host_port(parts.next()?)
}

fn parse_get_target(request_line: &str) -> Option<(String, u16, String)> {
    let mut parts = request_line.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    if target.starts_with("http://") {
        let without_scheme = target.trim_start_matches("http://");
        let (authority, path) = without_scheme
            .split_once('/')
            .map(|(host_port, path)| (host_port, format!("/{path}")))
            .unwrap_or((without_scheme, "/".to_string()));
        let (host, port) = parse_host_port(authority)?;
        return Some((host, port, path));
    }
    None
}

fn parse_host_port(authority: &str) -> Option<(String, u16)> {
    let (host, port) = authority.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
}

async fn proxy_connect(mut client: TcpStream, headers: &[u8], target: (String, u16)) {
    let (host, port) = target;
    let mut upstream = match TcpStream::connect((host.as_str(), port)).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%error, host, port, "sandbox proxy upstream connect failed");
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
            return;
        }
    };

    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }

    let body_offset = header_body_offset(headers);
    if body_offset < headers.len() && upstream.write_all(&headers[body_offset..]).await.is_err() {
        return;
    }

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

async fn proxy_get(mut client: TcpStream, headers: &[u8], host: String, port: u16, path: String) {
    let mut upstream = match TcpStream::connect((host.as_str(), port)).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%error, host, port, "sandbox proxy GET upstream connect failed");
            let _ = client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            return;
        }
    };

    let header_text = String::from_utf8_lossy(headers);
    let rewritten = rewrite_get_request(&header_text, &path);
    if upstream.write_all(rewritten.as_bytes()).await.is_err() {
        return;
    }

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

fn rewrite_get_request(headers: &str, path: &str) -> String {
    let mut lines = headers.lines();
    let Some(first_line) = lines.next() else {
        return String::new();
    };
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let _original_target = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("HTTP/1.1");
    let mut rewritten = format!("{method} {path} {version}\r\n");
    for line in lines {
        if line.is_empty() {
            break;
        }
        if line.to_ascii_lowercase().starts_with("proxy-connection:") {
            continue;
        }
        rewritten.push_str(line);
        rewritten.push_str("\r\n");
    }
    rewritten.push_str("\r\n");
    rewritten
}

/// Shared handle type used by the server runtime.
pub type SharedSandboxNetworkProxyHandle = Arc<SandboxNetworkProxyHandle>;

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_connect_target_extracts_host_and_port() {
        assert_eq!(
            parse_connect_target("CONNECT example.com:443 HTTP/1.1"),
            Some(("example.com".to_string(), 443))
        );
    }

    #[test]
    fn parse_get_target_extracts_absolute_url() {
        assert_eq!(
            parse_get_target("GET http://127.0.0.1:8080/hello HTTP/1.1"),
            Some(("127.0.0.1".to_string(), 8080, "/hello".to_string()))
        );
    }

    #[tokio::test]
    async fn proxy_forwards_get_through_local_echo_server() {
        let echo = TcpListener::bind("127.0.0.1:0").await.expect("bind echo");
        let echo_port = echo.local_addr().expect("echo addr").port();
        let echo_task = tokio::spawn(async move {
            let (mut stream, _) = echo.accept().await.expect("accept echo");
            let mut buf = [0u8; 1024];
            let read = stream.read(&mut buf).await.expect("read echo");
            stream.write_all(&buf[..read]).await.expect("write echo");
        });

        let proxy = start_sandbox_network_proxy().await.expect("start proxy");
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy.http_port()))
            .await
            .expect("connect proxy");
        client
            .write_all(
                format!(
                    "GET http://127.0.0.1:{echo_port}/ping HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .expect("write GET");

        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("read response");
        let response = String::from_utf8_lossy(&response);
        assert!(response.contains("GET /ping HTTP/1.1"));

        drop(proxy);
        echo_task.await.expect("echo task");
    }

    #[tokio::test]
    async fn proxy_connects_to_local_echo_server() {
        let echo = TcpListener::bind("127.0.0.1:0").await.expect("bind echo");
        let echo_port = echo.local_addr().expect("echo addr").port();
        let echo_task = tokio::spawn(async move {
            let (mut stream, _) = echo.accept().await.expect("accept echo");
            let mut buf = [0u8; 1024];
            let read = stream.read(&mut buf).await.expect("read echo");
            stream.write_all(&buf[..read]).await.expect("write echo");
        });

        let proxy = start_sandbox_network_proxy().await.expect("start proxy");
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy.http_port()))
            .await
            .expect("connect proxy");
        client
            .write_all(
                format!("CONNECT 127.0.0.1:{echo_port} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("write CONNECT");

        let mut established = [0u8; 64];
        let established_len = client
            .read(&mut established)
            .await
            .expect("read established");
        assert!(String::from_utf8_lossy(&established[..established_len]).contains("200"));

        client
            .write_all(b"proxy-payload")
            .await
            .expect("write payload");
        let mut payload = [0u8; 32];
        let read = client.read(&mut payload).await.expect("read payload");
        assert_eq!(&payload[..read], b"proxy-payload");

        drop(proxy);
        echo_task.await.expect("echo task");
    }
}
