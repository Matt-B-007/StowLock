// SPDX-License-Identifier: GPL-3.0-or-later
//! HTTPS without curl: ureq with rustls and the system's certificates, the
//! usual proxy settings (HTTPS_PROXY), a couple of retries for passing
//! network problems, and DNS-over-HTTPS when the system's DNS doesn't answer
//! (1.1.1.1, then 8.8.8.8; only the host name is sent, nothing else).

use crate::config::APP_VERSION;
use crate::util::Res;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// Biggest download accepted (the app is a few MB) and biggest text answer.
const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;
const MAX_TEXT: u64 = 20 * 1024 * 1024;

/// Resolve names with the system first; if that fails, ask DNS-over-HTTPS.
struct DohFallback;

impl ureq::Resolver for DohFallback {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        match netloc.to_socket_addrs() {
            Ok(a) => {
                let v: Vec<SocketAddr> = a.collect();
                if !v.is_empty() {
                    return Ok(v);
                }
                doh(netloc)
            }
            Err(_) => doh(netloc),
        }
    }
}

/// The addresses in a DNS-over-HTTPS JSON answer (type 1 = IPv4, 28 = IPv6).
pub fn parse_doh(json: &str) -> Vec<IpAddr> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    v["Answer"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|r| matches!(r["type"].as_u64(), Some(1) | Some(28)))
                .filter_map(|r| r["data"].as_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn doh(netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
    let (host, port) = netloc
        .rsplit_once(':')
        .and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?)))
        .unwrap_or((netloc, 443));
    let fail = || {
        std::io::Error::other(format!(
            "couldn't resolve {host} (system DNS and DNS-over-HTTPS)"
        ))
    };
    // Only plain host names, never anything that could change the query.
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(fail());
    }
    // IP addresses in the URLs: these requests need no DNS themselves.
    let plain = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent(&format!("StowLock/{APP_VERSION}"))
        .try_proxy_from_env(true)
        .build();
    for url in [
        format!("https://1.1.1.1/dns-query?name={host}&type=A"),
        format!("https://8.8.8.8/resolve?name={host}&type=A"),
    ] {
        let Ok(r) = plain.get(&url).set("accept", "application/dns-json").call() else {
            continue;
        };
        let mut body = String::new();
        if r.into_reader()
            .take(64 * 1024)
            .read_to_string(&mut body)
            .is_err()
        {
            continue;
        }
        let ips = parse_doh(&body);
        if !ips.is_empty() {
            return Ok(ips
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect());
        }
    }
    Err(fail())
}

fn build_agent(proxy: bool) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .user_agent(&format!("StowLock/{APP_VERSION}"))
        .try_proxy_from_env(proxy)
        .resolver(DohFallback)
        .build()
}

/// Should this host skip the proxy? Local addresses always do, and so does
/// anything NO_PROXY lists (ureq reads HTTPS_PROXY but not NO_PROXY).
pub fn bypass_proxy(host: &str, no_proxy: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host == "localhost" || host.starts_with("127.") || host == "::1" {
        return true;
    }
    no_proxy
        .split(',')
        .map(|e| e.trim().trim_start_matches('*').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .any(|e| {
            e == "*"
                || host == e.trim_start_matches('.')
                || host.ends_with(&format!(".{}", e.trim_start_matches('.')))
        })
}

fn agent_for(url: &str) -> &'static ureq::Agent {
    static PROXIED: OnceLock<ureq::Agent> = OnceLock::new();
    static DIRECT: OnceLock<ureq::Agent> = OnceLock::new();
    let host = url
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?']).next())
        .map(|hp| hp.rsplit_once(':').map(|(h, _)| h).unwrap_or(hp))
        .unwrap_or("");
    let no_proxy = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    if bypass_proxy(host, &no_proxy) {
        DIRECT.get_or_init(|| build_agent(false))
    } else {
        PROXIED.get_or_init(|| build_agent(true))
    }
}

/// Words the update messages look for ("404", "rate limit", "couldn't reach").
fn describe(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let limited = r.header("x-ratelimit-remaining") == Some("0");
            match code {
                404 => "HTTP 404 (not found)".into(),
                403 if limited => "HTTP 403 (rate limit)".into(),
                429 => "HTTP 429 (rate limit)".into(),
                c => format!("HTTP {c}"),
            }
        }
        ureq::Error::Transport(t) => format!("couldn't reach the server ({t})"),
    }
}

/// GET with up to 3 tries for passing problems (no answer, 5xx).
fn get(url: &str, secs: u64) -> Res<ureq::Response> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(attempt));
        }
        match agent_for(url)
            .get(url)
            .timeout(Duration::from_secs(secs))
            .call()
        {
            Ok(r) => return Ok(r),
            Err(ureq::Error::Status(code, r)) if code >= 500 => {
                last = describe(ureq::Error::Status(code, r));
            }
            Err(e @ ureq::Error::Status(..)) => return Err(describe(e)),
            Err(e) => last = describe(e),
        }
    }
    Err(last)
}

/// Fetch a URL as text.
pub fn get_text(url: &str, secs: u64) -> Res<String> {
    let r = get(url, secs)?;
    let mut body = String::new();
    r.into_reader()
        .take(MAX_TEXT)
        .read_to_string(&mut body)
        .map_err(|e| format!("couldn't read the answer: {e}"))?;
    Ok(body)
}

/// Download a URL into `out` (written in full, or not at all).
pub fn download(url: &str, out: &Path, secs: u64) -> Res<()> {
    let r = get(url, secs)?;
    let part = out.with_extension("part");
    let mut f =
        std::fs::File::create(&part).map_err(|e| format!("couldn't save the download: {e}"))?;
    let n = std::io::copy(&mut r.into_reader().take(MAX_DOWNLOAD + 1), &mut f)
        .map_err(|e| format!("the download stopped: {e}"))?;
    if n > MAX_DOWNLOAD {
        let _ = std::fs::remove_file(&part);
        return Err("The download is far bigger than expected, so it was stopped.".into());
    }
    std::fs::rename(&part, out).map_err(|e| format!("couldn't save the download: {e}"))
}

#[cfg(test)]
pub mod test_server {
    //! A tiny local web server for tests: serves files from a folder over
    //! plain HTTP on 127.0.0.1, and answers 404 for anything else.
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;

    pub fn serve(root: PathBuf) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                if r.read_line(&mut first).is_err() {
                    continue;
                }
                let mut line = String::new();
                while r.read_line(&mut line).map(|n| n > 2).unwrap_or(false) {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let file = root.join(path.trim_start_matches('/').split('?').next().unwrap_or(""));
                let mut s = s;
                match std::fs::read(&file).ok().filter(|_| file.is_file()) {
                    Some(body) => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = s.write_all(&body);
                    }
                    None => {
                        let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            }
        });
        format!("http://{addr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetches_text_and_files_without_curl() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("hello.txt"), "hi there").unwrap();
        std::fs::write(d.path().join("app.bin"), vec![7u8; 100_000]).unwrap();
        let base = test_server::serve(d.path().to_path_buf());
        assert_eq!(
            get_text(&format!("{base}/hello.txt"), 10).unwrap(),
            "hi there"
        );
        let out = d.path().join("got.bin");
        download(&format!("{base}/app.bin"), &out, 10).unwrap();
        assert_eq!(std::fs::read(&out).unwrap().len(), 100_000);
        assert!(!out.with_extension("part").exists());
        let e = get_text(&format!("{base}/missing"), 10).unwrap_err();
        assert!(e.contains("404"), "{e}");
    }

    #[test]
    fn reads_dns_over_https_answers() {
        let json = r#"{"Status":0,"Answer":[{"name":"github.com","type":5,"data":"x.github.com."},{"name":"x.github.com","type":1,"data":"140.82.112.3"}]}"#;
        assert_eq!(parse_doh(json), ["140.82.112.3".parse::<IpAddr>().unwrap()]);
        assert!(parse_doh("not json").is_empty());
    }

    #[test]
    fn odd_host_names_never_reach_dns_over_https() {
        assert!(doh("github.com&type=TXT:443").is_err());
    }

    #[test]
    fn local_and_no_proxy_hosts_skip_the_proxy() {
        assert!(bypass_proxy("127.0.0.1", ""));
        assert!(bypass_proxy("localhost", ""));
        assert!(bypass_proxy("api.example.com", "foo.org,.example.com"));
        assert!(bypass_proxy("example.com", "example.com"));
        assert!(!bypass_proxy("github.com", "example.com"));
        assert!(!bypass_proxy("notexample.com", "example.com"));
    }
}
