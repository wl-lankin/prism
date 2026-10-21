//! Fake servers for the radio tests, hand-rolled on a plain `TcpListener` bound to `127.0.0.1`
//! (one request per connection, `Connection: close`), like the fake Subsonic server.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// Reads the request head and returns the path without its query.
fn read_path(stream: &mut TcpStream) -> Option<String> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 || head.len() > 16 * 1024 {
            return None;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8(head).ok()?;
    let target = head.lines().next()?.split(' ').nth(1)?.to_owned();
    Some(target.split('?').next().unwrap_or("/").to_owned())
}

fn write_reply(stream: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    // A client that gave up is not the server's problem.
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

// ---- FakeGeo ----------------------------------------------------------------------------

/// Path of the fake GeoJS endpoint.
const GEOJS_PATH: &str = "/v1/ip/geo.json";
/// Path of the fake ipinfo endpoint.
const IPINFO_PATH: &str = "/json";

struct GeoState {
    answers: HashMap<&'static str, (u16, String)>,
    hits: HashMap<&'static str, usize>,
}

/// A fake location service with the two endpoints Prism asks. Both answer the Testhausen
/// fixtures until a test changes them. Stops when dropped.
pub(crate) struct FakeGeo {
    port: u16,
    state: Arc<Mutex<GeoState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeGeo {
    pub fn start() -> FakeGeo {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the fake geo server");
        let port = listener.local_addr().expect("fake geo address").port();
        listener
            .set_nonblocking(true)
            .expect("non-blocking fake geo server");
        let state = Arc::new(Mutex::new(GeoState {
            answers: HashMap::from([
                (
                    GEOJS_PATH,
                    (
                        200,
                        include_str!("../../tests/fixtures/radio/geojs.json").to_owned(),
                    ),
                ),
                (
                    IPINFO_PATH,
                    (
                        200,
                        include_str!("../../tests/fixtures/radio/ipinfo.json").to_owned(),
                    ),
                ),
            ]),
            hits: HashMap::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let state = Arc::clone(&state);
                            std::thread::spawn(move || {
                                let Some(path) = read_path(&mut stream) else {
                                    return;
                                };
                                let answer = {
                                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                                    let key = [GEOJS_PATH, IPINFO_PATH]
                                        .into_iter()
                                        .find(|known| *known == path);
                                    key.map(|key| {
                                        *state.hits.entry(key).or_default() += 1;
                                        state.answers[key].clone()
                                    })
                                };
                                match answer {
                                    Some((status, body)) => write_reply(&mut stream, status, &body),
                                    None => write_reply(&mut stream, 404, "{}"),
                                }
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        FakeGeo {
            port,
            state,
            stop,
            thread: Some(thread),
        }
    }

    pub fn geojs_url(&self) -> String {
        format!("http://127.0.0.1:{}{GEOJS_PATH}", self.port)
    }

    pub fn ipinfo_url(&self) -> String {
        format!("http://127.0.0.1:{}{IPINFO_PATH}", self.port)
    }

    pub fn set_geojs(&self, status: u16, body: &str) {
        self.set(GEOJS_PATH, status, body);
    }

    pub fn set_ipinfo(&self, status: u16, body: &str) {
        self.set(IPINFO_PATH, status, body);
    }

    fn set(&self, path: &'static str, status: u16, body: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.answers.insert(path, (status, body.to_owned()));
    }

    pub fn geojs_hits(&self) -> usize {
        self.hits(GEOJS_PATH)
    }

    pub fn ipinfo_hits(&self) -> usize {
        self.hits(IPINFO_PATH)
    }

    fn hits(&self, path: &str) -> usize {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.hits.get(path).copied().unwrap_or(0)
    }
}

impl Drop for FakeGeo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
