//! Socket smoke test: a Minecraft server-list ping to a numeric address, with no DNS and no socket options
//! (Cemu's nsysnet lacks getaddrinfo and crashes in setsockopt).
extern crate wiiu_atomics;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

fn varint(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

fn packet(id: u32, body: &[u8]) -> Vec<u8> {
    let mut inner = Vec::new();
    varint(id, &mut inner);
    inner.extend_from_slice(body);
    let mut out = Vec::new();
    varint(inner.len() as u32, &mut out);
    out.extend(inner);
    out
}

fn main() {
    println!("nettest: start");
    let addr: SocketAddr = "127.0.0.1:25565".parse().unwrap();
    let mut s = match TcpStream::connect(addr) {
        Ok(s) => s,
        Err(e) => {
            println!("nettest: connect failed: {e}");
            return;
        }
    };
    println!("nettest: connected");
    // handshake: protocol 777 (as the game states it), host, port, next state 1 (status)
    let mut hs = Vec::new();
    varint(777, &mut hs);
    varint(9, &mut hs);
    hs.extend_from_slice(b"127.0.0.1");
    hs.extend_from_slice(&25565u16.to_be_bytes());
    varint(1, &mut hs);
    let _ = s.write_all(&packet(0, &hs));
    let _ = s.write_all(&packet(0, &[]));
    let mut buf = vec![0u8; 2048];
    match s.read(&mut buf) {
        Ok(n) => println!("nettest: got {n} bytes: {}", String::from_utf8_lossy(&buf[..n.min(160)]).replace('\u{0}', ".")),
        Err(e) => println!("nettest: read failed: {e}"),
    }
}
