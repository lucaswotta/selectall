//! Varredura da porta do banco: quem aceita conexão TCP e, no MySQL/MariaDB,
//! a versão anunciada no pacote de saudação. Não autentica.

use serde::Serialize;
use std::io::Read;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub const MAX_CONCURRENCY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScanHit {
    pub host: String,
    pub open: bool,
    /// Versão anunciada pelo servidor MySQL/MariaDB; ausente em outros motores.
    pub version: Option<String>,
}

/// Testa cada endereço com até `concurrency` conexões simultâneas. Mantém a ordem da entrada.
pub fn scan(hosts: &[String], port: u16, timeout: Duration, concurrency: usize) -> Vec<ScanHit> {
    let results: Vec<Mutex<Option<ScanHit>>> = hosts.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let workers = concurrency.clamp(1, MAX_CONCURRENCY).min(hosts.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(host) = hosts.get(index) else { break };
                    let hit = probe(host, port, timeout);
                    *results[index]
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hit);
                }
            });
        }
    });
    results
        .into_iter()
        .zip(hosts)
        .map(|(slot, host)| {
            slot.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .unwrap_or_else(|| closed(host))
        })
        .collect()
}

fn closed(host: &str) -> ScanHit {
    ScanHit {
        host: host.to_owned(),
        open: false,
        version: None,
    }
}

fn probe(host: &str, port: u16, timeout: Duration) -> ScanHit {
    let Some(address) = resolve(host, port) else {
        return closed(host);
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
        return closed(host);
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let mut greeting = [0u8; 128];
    let mut length = 0;
    // A saudação pode chegar em mais de um segmento; lê até o fim da versão ou o prazo.
    while length < greeting.len() {
        match stream.read(&mut greeting[length..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => length += read,
        }
        if greeting[..length].iter().skip(5).any(|byte| *byte == 0) {
            break;
        }
    }
    ScanHit {
        host: host.to_owned(),
        open: true,
        version: mysql_version(&greeting[..length]),
    }
}

fn resolve(host: &str, port: u16) -> Option<SocketAddr> {
    (host, port).to_socket_addrs().ok()?.next()
}

/// Lê a versão do pacote de saudação do protocolo 10: cabeçalho de 4 bytes, `0x0a`, texto terminado em zero.
fn mysql_version(packet: &[u8]) -> Option<String> {
    let payload = packet.get(4..)?;
    if payload.first() != Some(&10) {
        return None;
    }
    let text = payload.get(1..)?;
    let end = text.iter().position(|byte| *byte == 0)?;
    let version = std::str::from_utf8(&text[..end]).ok()?.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    fn greeting(version: &str) -> Vec<u8> {
        let mut payload = vec![10];
        payload.extend_from_slice(version.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&[1, 0, 0, 0]);
        let length = u32::try_from(payload.len()).unwrap().to_le_bytes();
        let mut packet = vec![length[0], length[1], length[2], 0];
        packet.extend(payload);
        packet
    }

    #[test]
    fn parses_greeting_and_ignores_other_protocols() {
        assert_eq!(mysql_version(&greeting("8.4.3")).as_deref(), Some("8.4.3"));
        assert_eq!(mysql_version(&[5, 0, 0, 0, 0xff, 0x6a, 0x04]), None);
        assert_eq!(mysql_version(b""), None);
        assert_eq!(mysql_version(&[1, 0, 0, 0, 10, b'8']), None);
    }

    #[test]
    fn finds_open_ports_and_their_versions() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Em dois segmentos, como pode acontecer na rede.
            let packet = greeting("5.7.44-log");
            stream.write_all(&packet[..6]).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            stream.write_all(&packet[6..]).unwrap();
        });
        // O listener escuta só em 127.0.0.1, então a mesma porta em 127.0.0.2 está fechada.
        let hosts = vec![
            "127.0.0.1".to_owned(),
            "127.0.0.2".to_owned(),
            "host.invalido.".to_owned(),
        ];
        let hits = scan(&hosts, port, Duration::from_millis(500), 4);
        server.join().unwrap();
        assert_eq!(hits[0].version.as_deref(), Some("5.7.44-log"));
        assert!(hits[0].open);
        assert!(!hits[1].open);
        assert!(!hits[2].open);
        assert_eq!(
            hits.iter().map(|hit| hit.host.as_str()).collect::<Vec<_>>(),
            ["127.0.0.1", "127.0.0.2", "host.invalido."]
        );
    }

    #[test]
    fn empty_input_returns_nothing() {
        assert_eq!(scan(&[], 3306, Duration::from_millis(10), 8), []);
    }
}
