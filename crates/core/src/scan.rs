//! Varredura da porta do banco: quem aceita conexão TCP e qual motor responde.
//! O MySQL/MariaDB fala primeiro e anuncia a versão; o PostgreSQL responde ao pedido
//! de TLS com um byte; o SQL Server responde ao `PRELOGIN` com a versão. Não autentica.

use crate::model::Engine;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub const MAX_CONCURRENCY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScanHit {
    pub host: String,
    pub open: bool,
    /// Motor que respondeu ao protocolo; ausente quando a porta abre e ninguém responde.
    pub engine: Option<Engine>,
    /// Versão anunciada antes do login: MySQL/MariaDB e SQL Server. O PostgreSQL não anuncia.
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
        engine: None,
        version: None,
    }
}

/// O motor que responde na porta padrão é tentado primeiro: cada tentativa abre outra
/// conexão, e o MySQL sem saudação só desiste no prazo.
fn probe(host: &str, port: u16, timeout: Duration) -> ScanHit {
    let Some(address) = resolve(host, port) else {
        return closed(host);
    };
    let order = match port {
        5432 => [Engine::Postgres, Engine::Sqlserver, Engine::Mysql],
        1433 => [Engine::Sqlserver, Engine::Postgres, Engine::Mysql],
        _ => [Engine::Mysql, Engine::Postgres, Engine::Sqlserver],
    };
    let mut open = false;
    for engine in order {
        let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
            break;
        };
        open = true;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
        let found = match engine {
            Engine::Mysql => mysql_version(&read_greeting(&mut stream)).map(Some),
            Engine::Postgres => postgres_answers(&mut stream).then_some(None),
            Engine::Sqlserver => {
                sqlserver_prelogin(&mut stream).map(|payload| prelogin_version(&payload))
            }
        };
        if let Some(version) = found {
            return ScanHit {
                host: host.to_owned(),
                open: true,
                engine: Some(engine),
                version,
            };
        }
    }
    ScanHit {
        host: host.to_owned(),
        open,
        engine: None,
        version: None,
    }
}

fn read_greeting(stream: &mut TcpStream) -> Vec<u8> {
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
    greeting[..length].to_vec()
}

/// `SSLRequest`: o PostgreSQL responde `S` ou `N` antes de qualquer autenticação.
fn postgres_answers(stream: &mut TcpStream) -> bool {
    const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xD2, 0x16, 0x2F];
    let mut answer = [0u8; 1];
    stream.write_all(&SSL_REQUEST).is_ok()
        && stream.read_exact(&mut answer).is_ok()
        && matches!(answer[0], b'S' | b'N')
}

/// Pacote `PRELOGIN` do TDS com as opções de versão e de criptografia.
const PRELOGIN: [u8; 26] = [
    0x12, 0x01, 0x00, 0x1A, 0x00, 0x00, 0x01,
    0x00, // cabeçalho: PRELOGIN, último pacote, 26 bytes
    0x00, 0x00, 0x0B, 0x00, 0x06, // VERSION no deslocamento 11, 6 bytes
    0x01, 0x00, 0x11, 0x00, 0x01, // ENCRYPTION no deslocamento 17, 1 byte
    0xFF, // fim das opções
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // versão do cliente: zero
    0x02, // sem criptografia
];

/// Corpo da resposta ao `PRELOGIN`, quando quem responde fala TDS.
fn sqlserver_prelogin(stream: &mut TcpStream) -> Option<Vec<u8>> {
    stream.write_all(&PRELOGIN).ok()?;
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).ok()?;
    // Resposta tabular (0x04) com o tamanho total no cabeçalho.
    if header[0] != 0x04 {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    let mut payload = vec![0u8; length.checked_sub(8)?.min(4096)];
    stream.read_exact(&mut payload).ok()?;
    Some(payload)
}

/// Versão `major.minor.build` da opção `VERSION` da resposta ao `PRELOGIN`.
fn prelogin_version(payload: &[u8]) -> Option<String> {
    let mut option = 0;
    while let Some(&token) = payload.get(option) {
        if token == 0xFF {
            return None;
        }
        let field = payload.get(option + 1..option + 5)?;
        let offset = usize::from(u16::from_be_bytes([field[0], field[1]]));
        let length = usize::from(u16::from_be_bytes([field[2], field[3]]));
        if token == 0x00 && length >= 4 {
            let version = payload.get(offset..offset + 4)?;
            let build = u16::from_be_bytes([version[2], version[3]]);
            return Some(format!("{}.{}.{build}", version[0], version[1]));
        }
        option += 5;
    }
    None
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
    fn reads_the_sqlserver_prelogin_version() {
        // Opções VERSION (16.0.4135) e ENCRYPTION, como o SQL Server 2022 responde.
        let payload = [
            0x00, 0x00, 0x0B, 0x00, 0x06, 0x01, 0x00, 0x11, 0x00, 0x01, 0xFF, 16, 0, 0x10, 0x27, 0,
            0, 0x02,
        ];
        assert_eq!(prelogin_version(&payload).as_deref(), Some("16.0.4135"));
        assert_eq!(prelogin_version(&[0xFF]), None);
        assert_eq!(prelogin_version(&[0x00, 0x00]), None);
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
        assert_eq!(hits[0].engine, Some(Engine::Mysql));
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
