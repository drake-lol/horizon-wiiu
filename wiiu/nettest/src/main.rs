extern crate wiiu_atomics;

use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};

/// Host names from `/vol/external01/horizon/hosts.txt` ("name ip" per line) win over system DNS; Cemu has no
/// working `getaddrinfo`, and a console on a bad DNS setting can be pointed at the right address by hand.
struct HostsResolver;

impl ureq::Resolver for HostsResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let (host, port) = netloc.rsplit_once(':').unwrap_or((netloc, "443"));
        if let Ok(text) = std::fs::read_to_string("/vol/external01/horizon/hosts.txt") {
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                if let (Some(name), Some(ip)) = (parts.next(), parts.next()) {
                    if name == host {
                        if let Ok(addr) = format!("{ip}:{port}").parse::<SocketAddr>() {
                            return Ok(vec![addr]);
                        }
                    }
                }
            }
        }
        netloc.to_socket_addrs().map(|a| a.collect())
    }
}

fn main() {
    println!("nettest: start");
    let agent = ureq::AgentBuilder::new().resolver(HostsResolver).build();
    let url = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
    match agent.get(url).call() {
        Ok(resp) => {
            println!("nettest: HTTP {}", resp.status());
            let mut body = String::new();
            let n = resp.into_reader().take(4096).read_to_string(&mut body).unwrap_or(0);
            println!("nettest: {n} bytes: {}", body.chars().take(120).collect::<String>());
        }
        Err(e) => println!("nettest: error: {e}"),
    }
}
