//! The real Horizon / Minecraft Java 26.3 network protocol (`horizon-protocol`): join a server offline, finish
//! configuration, then keep the connection alive and turn the chunks it sends into blocks.
//!
//! The socket is used through `&TcpStream` from two threads (no `try_clone`: Wii U sockets are not duplicable file
//! descriptors) and no socket options are set (Cemu's nsysnet crashes in `setsockopt`). A numeric address is used;
//! `server.txt` on the SD card names it. Block state ids become the viewer's `Block` through `states.bin`
//! (`wiiu/mkstates` makes it from the player's own prepared data).

use horizon_protocol::{
    chunk::{SectionLight, nibble},
    PROTOCOL_VERSION,
    buf::{Reader, Writer},
    chunk::Section,
    crypto::offline_uuid,
    frame, ids,
    packets::{
        Empty, configuration,
        handshake::{self, Intention},
        login::{self, ServerHello},
        play,
    },
    registry::{Dynamic, ceil_log2},
};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// A live connection: events in, the player's position out.
pub struct Net {
    pub events: Receiver<Event>,
    shared: Arc<Shared>,
}

/// Set once `PlayerLoaded` has been sent: the server ignores a player's moves until then.
static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
static LAST_CHUNK_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// No chunk has arrived for `ms`: the server has finished sending (and generating) the area around the player.
pub fn chunks_quiet(ms: u64) -> bool {
    LOADED.load(Ordering::SeqCst) && now_ms().saturating_sub(LAST_CHUNK_MS.load(Ordering::SeqCst)) >= ms
}

pub fn loaded() -> bool {
    LOADED.load(Ordering::SeqCst)
}

impl Net {
    /// Tell the server where the player is (`MovePlayerPos`), so it keeps their chunks loaded and sees them move.
    pub fn move_to(&self, pos: [f64; 3], on_ground: bool) {
        static SENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SENT.fetch_add(1, Ordering::Relaxed);
        if n < 12 || n % 60 == 0 {
            // integers, not floats: the float formatter misbehaves on this target (see wiiu/README.md)
            println!("net: move #{n} to ({}, {}, {}) milli-blocks ground={on_ground}", (pos[0] * 1000.0) as i64, (pos[1] * 1000.0) as i64, (pos[2] * 1000.0) as i64);
        }
        let body = play::Serverbound::MovePlayerPos(play::MovePlayerPos { x: pos[0], y: pos[1], z: pos[2], flags: on_ground as u8 }).encode();
        let _ = self.shared.send(&body);
    }
}

pub enum Event {
    /// The first position the server put the player at.
    Spawn([f64; 3]),
    /// A chunk column: block ids per 16-high section from the lowest one (`4096` bytes each, y-major).
    Chunk { x: i32, z: i32, min_y: i32, sections: Vec<Vec<u16>>, light: Vec<Vec<u8>> },
    Block { pos: [i32; 3], block: u16 },
    Closed(String),
}

struct Shared {
    stream: TcpStream,
    /// -1: no compression.
    threshold: AtomicI32,
    write_lock: Mutex<()>,
}

impl Shared {
    fn threshold(&self) -> Option<usize> {
        usize::try_from(self.threshold.load(Ordering::SeqCst)).ok()
    }

    fn send(&self, body: &[u8]) -> std::io::Result<()> {
        let frame = frame::encode(body, self.threshold()).map_err(|e| std::io::Error::other(e.to_string()))?;
        let _guard = self.write_lock.lock().unwrap();
        (&self.stream).write_all(&frame)
    }
}

struct Framer {
    shared: Arc<Shared>,
    buf: Vec<u8>,
    inflater: frame::Inflater,
}

impl Framer {
    /// The next packet: (id, payload after the id).
    fn recv(&mut self) -> std::io::Result<(i32, Vec<u8>)> {
        loop {
            let bad = |e: horizon_protocol::Error| std::io::Error::other(e.to_string());
            if let Some((head, len)) = frame::header(&self.buf).map_err(bad)? {
                if self.buf.len() >= head + len {
                    let frame_bytes: Vec<u8> = self.buf[head..head + len].to_vec();
                    self.buf.drain(..head + len);
                    let payload = match frame::body(&frame_bytes, self.shared.threshold(), &mut self.inflater).map_err(bad)? {
                        frame::Body::Plain(at) => frame_bytes[at..].to_vec(),
                        frame::Body::Inflated(v) => v,
                    };
                    let mut r = Reader::new(&payload);
                    let id = r.varint().map_err(bad)?;
                    let at = r.position();
                    return Ok((id, payload[at..].to_vec()));
                }
            }
            let mut tmp = [0u8; 4096];
            let n = (&self.shared.stream).read(&mut tmp)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn raw(id: i32, write: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    w.varint(id);
    write(&mut w);
    w.data
}

/// The server's address: `server.txt` on the SD card, else the default (a Horizon server on this machine).
pub fn server_address() -> SocketAddr {
    std::fs::read_to_string("/vol/external01/horizon/server.txt")
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or_else(|| "127.0.0.1:25565".parse().unwrap())
}

/// Block state id -> block type id: `HZS2` (u16 per state, from mkatlas) or the older `HZST` (one placeholder block
/// byte per state, from mkstates).
fn load_states() -> Vec<u16> {
    match std::fs::read("/vol/external01/horizon/states.bin") {
        Ok(b) if b.len() >= 8 && &b[..4] == b"HZS2" => b[8..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect(),
        Ok(b) if b.len() >= 8 && &b[..4] == b"HZST" => b[8..].iter().map(|&v| v as u16).collect(),
        _ => Vec::new(),
    }
}

/// Join `addr` as `name` (offline mode) and return the receiver of the events the connection produces.
pub fn join(addr: SocketAddr, name: &str, view_distance: u8) -> Result<Net, String> {
    let stream = TcpStream::connect(addr).map_err(err)?;
    let shared = Arc::new(Shared { stream, threshold: AtomicI32::new(-1), write_lock: Mutex::new(()) });
    let mut framer = Framer { shared: shared.clone(), buf: Vec::new(), inflater: frame::Inflater::default() };

    shared
        .send(&handshake::Serverbound::Intention(Intention { protocol: PROTOCOL_VERSION, host: addr.ip().to_string(), port: addr.port(), intent: handshake::LOGIN }).encode())
        .map_err(err)?;
    shared.send(&login::Serverbound::Hello(ServerHello { name: name.into(), uuid: offline_uuid(name) }).encode()).map_err(err)?;
    loop {
        let (id, body) = framer.recv().map_err(err)?;
        match login::Clientbound::decode(id, &body).map_err(err)? {
            login::Clientbound::Compression(p) => shared.threshold.store(p.threshold, Ordering::SeqCst),
            login::Clientbound::LoginFinished(_) => {
                shared.send(&login::Serverbound::LoginAcknowledged(Empty).encode()).map_err(err)?;
                break;
            }
            login::Clientbound::Disconnect(p) => return Err(format!("refused: {}", p.reason_json)),
            login::Clientbound::Hello(_) => return Err("the server is in online mode".into()),
            login::Clientbound::CustomQuery(p) => shared
                .send(&login::Serverbound::CustomQueryAnswer(login::CustomQueryAnswer { transaction: p.transaction, payload: None }).encode())
                .map_err(err)?,
            login::Clientbound::CookieRequest(p) => shared
                .send(&login::Serverbound::CookieResponse(login::CookieResponse { key: p.key, payload: None }).encode())
                .map_err(err)?,
            login::Clientbound::Unknown(..) => {}
        }
    }

    // configuration
    use ids::configuration::clientbound as cb;
    shared.send(&configuration::Serverbound::ClientInformation(configuration::ClientInformation::new(view_distance as i8)).encode()).map_err(err)?;
    let mut w = Writer::new();
    w.string("horizon");
    shared
        .send(&configuration::Serverbound::CustomPayload(configuration::CustomPayload { channel: "minecraft:brand".into(), data: w.data }).encode())
        .map_err(err)?;
    let mut dynamic = Dynamic::default();
    loop {
        let (id, body) = framer.recv().map_err(err)?;
        match configuration::Clientbound::decode(id, &body) {
            Ok(configuration::Clientbound::SelectKnownPacks(p)) => shared.send(&configuration::Serverbound::SelectKnownPacks(p).encode()).map_err(err)?,
            Ok(configuration::Clientbound::KeepAlive(p)) => shared.send(&configuration::Serverbound::KeepAlive(p).encode()).map_err(err)?,
            Ok(configuration::Clientbound::Ping(p)) => shared.send(&configuration::Serverbound::Pong(p).encode()).map_err(err)?,
            Ok(configuration::Clientbound::CodeOfConduct(_)) => shared.send(&configuration::Serverbound::AcceptCodeOfConduct(Empty).encode()).map_err(err)?,
            Ok(configuration::Clientbound::CookieRequest(p)) => shared
                .send(&configuration::Serverbound::CookieResponse(configuration::CookieResponse { key: p.key, payload: None }).encode())
                .map_err(err)?,
            Ok(configuration::Clientbound::RegistryData(p)) => dynamic.add(p),
            Ok(configuration::Clientbound::Disconnect(p)) => return Err(format!("disconnected during configuration: {}", horizon_protocol::text::plain(&p.reason, &Default::default()))),
            Ok(configuration::Clientbound::FinishConfiguration(_)) => {
                shared.send(&configuration::Serverbound::FinishConfiguration(Empty).encode()).map_err(err)?;
                break;
            }
            _ => {
                let _ = cb::FINISH_CONFIGURATION;
            }
        }
    }
    let biome_bits = ceil_log2(dynamic.len("minecraft:worldgen/biome").max(1));
    println!("net: configuration done ({} biomes, {biome_bits} biome bits)", dynamic.len("minecraft:worldgen/biome"));

    let (tx, rx) = mpsc::channel();
    let states = load_states();
    println!("net: {} block states in states.bin", states.len());
    let for_thread = shared.clone();
    std::thread::spawn(move || play(framer, for_thread, dynamic, biome_bits, states, tx));
    Ok(Net { events: rx, shared })
}

fn play(mut framer: Framer, shared: Arc<Shared>, dynamic: Dynamic, biome_bits: u8, states: Vec<u16>, tx: Sender<Event>) {
    use ids::play::clientbound as cb;
    use ids::play::serverbound as sb;
    let mut dimension = dynamic.dimension(0);
    let mut batch_started = std::time::Instant::now();
    let mut chunks = 0u32;
    // The server resends a teleport it has not seen answered; answering the same id twice is a kick.
    let mut accepted: Option<i32> = None;
    let block_bits = ceil_log2(states.len().max(2));
    let close = |tx: &Sender<Event>, why: String| {
        let _ = tx.send(Event::Closed(why));
    };
    loop {
        let (id, body) = match framer.recv() {
            Ok(p) => p,
            Err(e) => return close(&tx, e.to_string()),
        };
        let mut r = Reader::new(&body);
        let sent = match id {
            cb::KEEP_ALIVE => match r.i64() {
                Ok(v) => shared.send(&raw(sb::KEEP_ALIVE, |w| w.i64(v))),
                Err(_) => Ok(()),
            },
            cb::PING => match r.i32() {
                Ok(v) => shared.send(&raw(sb::PONG, |w| w.i32(v))),
                Err(_) => Ok(()),
            },
            cb::LOGIN => {
                if let Ok(play::Clientbound::Login(p)) = play::Clientbound::decode(id, &body) {
                    dimension = dynamic.dimension(p.spawn.dimension_type);
                    println!("net: joined {} ({} sections from y {})", p.spawn.dimension, dimension.sections(), dimension.min_y);
                }
                Ok(())
            }
            cb::CHUNK_BATCH_START => {
                batch_started = std::time::Instant::now();
                Ok(())
            }
            cb::CHUNK_BATCH_FINISHED => {
                let _ = batch_started;
                let r = shared.send(&play::Serverbound::ChunkBatchReceived(play::ChunkBatchReceived { chunks_per_tick: 9.0 }).encode());
                // vanilla's loading screen ends, and the client says so, once the first chunks are in
                if r.is_ok() && !LOADED.load(Ordering::SeqCst) {
                    LOADED.store(true, Ordering::SeqCst);
                    println!("net: sending PlayerLoaded");
                    shared.send(&play::Serverbound::PlayerLoaded(Empty).encode())
                } else {
                    r
                }
            }
            cb::PLAYER_POSITION => match play::Clientbound::decode(id, &body) {
                Ok(play::Clientbound::PlayerPosition(p)) => {
                    println!("net: teleport #{} (already answered: {})", p.teleport, accepted == Some(p.teleport));
                    if accepted == Some(p.teleport) {
                        continue;
                    }
                    accepted = Some(p.teleport);
                    let _ = tx.send(Event::Spawn(p.position));
                    shared.send(
                        &play::Serverbound::AcceptTeleportation(play::AcceptTeleportation {
                            id: p.teleport,
                            x: p.position[0],
                            y: p.position[1],
                            z: p.position[2],
                            yaw: p.yaw,
                            pitch: p.pitch,
                        })
                        .encode(),
                    )
                }
                _ => Ok(()),
            },
            cb::LEVEL_CHUNK_WITH_LIGHT => {
                if let Ok(play::Clientbound::LevelChunkWithLight(p)) = play::Clientbound::decode(id, &body) {
                    match p.chunk.sections(dimension.sections(), block_bits, biome_bits) {
                        Ok(secs) => {
                            chunks += 1;
                            LAST_CHUNK_MS.store(now_ms(), Ordering::SeqCst);
                            let sections = secs.iter().map(|s| section_blocks(s, &states)).collect();
                            // light sections start one below the lowest block section
                            let layers = p.light.layers(dimension.sections() + 2);
                            let light = (0..dimension.sections()).map(|si| section_light(layers.get(si + 1))).collect();
                            let _ = tx.send(Event::Chunk { x: p.x, z: p.z, min_y: dimension.min_y, sections, light });
                        }
                        Err(e) => println!("net: chunk {},{} did not decode: {e}", p.x, p.z),
                    }
                }
                Ok(())
            }
            cb::BLOCK_UPDATE => {
                if let Ok(play::Clientbound::BlockUpdate(p)) = play::Clientbound::decode(id, &body) {
                    let b = states.get(p.state as usize).copied().unwrap_or(1u16);
                    let _ = tx.send(Event::Block { pos: p.pos, block: b });
                }
                Ok(())
            }
            cb::DISCONNECT | cb::START_CONFIGURATION => return close(&tx, format!("server closed the game (packet {id})")),
            _ => Ok(()),
        };
        if let Err(e) = sent {
            return close(&tx, e.to_string());
        }
        if chunks > 0 && chunks % 25 == 0 && id == cb::LEVEL_CHUNK_WITH_LIGHT {
            println!("net: {chunks} chunks received");
        }
    }
}

/// One section's block ids for the viewer, from its paletted states.
fn section_blocks(s: &Section, states: &[u16]) -> Vec<u16> {
    let mut out = vec![0u16; 4096];
    if s.non_empty == 0 {
        return out;
    }
    for (i, slot) in out.iter_mut().enumerate() {
        let state = s.states.get(i);
        *slot = states.get(state as usize).copied().unwrap_or(1);
    }
    out
}

/// The light a block gets: the brighter of sky and block light, 0..15. A layer the server left out is full sky
/// light (above the terrain) and no block light.
fn section_light(layers: Option<&SectionLight>) -> Vec<u8> {
    let mut out = vec![15u8; 4096];
    let Some((sky, block)) = layers else { return out };
    for (i, slot) in out.iter_mut().enumerate() {
        let s = sky.as_ref().map_or(15, |l| nibble(l, i));
        let b = block.as_ref().map_or(0, |l| nibble(l, i));
        *slot = s.max(b);
    }
    out
}
