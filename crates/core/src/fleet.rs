//! Cadastro em lote: regra de endereçamento, colar lista e o plano de inclusão
//! com prévia, duplicados e erros por linha. Planilha e varredura geram o mesmo
//! [`Batch`], que vira um [`Plan`] antes de tocar no banco.

use crate::model::{AddressRule, Device, Profile, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::Ipv4Addr;

/// Tipo usado quando nem a entrada nem a regra dizem qual é.
pub const DEFAULT_KIND: &str = "PDV";

/// Loja citada numa entrada em lote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreSpec {
    pub line: usize,
    pub code: String,
    pub name: String,
    pub subnet: String,
}

/// Equipamento candidato. Campos vazios são deduzidos da regra da rede no plano.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub line: usize,
    pub store_code: Option<String>,
    pub host: String,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub profile_id: Option<i64>,
    pub profile_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineError {
    pub line: usize,
    pub message: String,
}

impl LineError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

/// O que veio de uma fonte (regra, lista, planilha, varredura), ainda sem conferir a frota.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub stores: Vec<StoreSpec>,
    pub entries: Vec<Entry>,
    pub errors: Vec<LineError>,
    /// Avisos que não impedem a inclusão, como uma coluna de senha ignorada.
    pub notices: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceStatus {
    /// Entra na frota.
    New,
    /// O endereço já está cadastrado nesta rede.
    Exists,
    /// O endereço apareceu antes nesta mesma entrada.
    Repeated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedDevice {
    pub line: usize,
    pub store_code: String,
    pub name: String,
    pub kind: String,
    pub host: String,
    pub profile_ids: Vec<i64>,
    pub status: DeviceStatus,
}

/// Prévia do que será gravado. Só lojas novas aparecem em `stores`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub stores: Vec<StoreSpec>,
    pub devices: Vec<PlannedDevice>,
    pub errors: Vec<LineError>,
    pub notices: Vec<String>,
}

impl Plan {
    pub fn new_devices(&self) -> impl Iterator<Item = &PlannedDevice> {
        self.devices
            .iter()
            .filter(|device| device.status == DeviceStatus::New)
    }
}

/// Endereço IPv4 com faixa opcional no último octeto: `10.113.3.1-12`.
pub fn expand_range(text: &str) -> Result<Vec<String>, String> {
    let text = text.trim();
    let invalid = || format!("Endereço inválido: {text}. Use 10.25.3.14 ou 10.25.3.1-50.");
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return Err(invalid());
    }
    let mut prefix = [0u8; 3];
    for (slot, part) in prefix.iter_mut().zip(&parts) {
        *slot = parse_octet(part).ok_or_else(invalid)?;
    }
    let (first, last) = parts[3].split_once('-').unwrap_or((parts[3], parts[3]));
    let (first, last) = (
        parse_octet(first).ok_or_else(invalid)?,
        parse_octet(last).ok_or_else(invalid)?,
    );
    if first > last {
        return Err(format!(
            "Faixa invertida em {text}: o início deve ser menor que o fim."
        ));
    }
    Ok((first..=last)
        .map(|octet| Ipv4Addr::new(prefix[0], prefix[1], prefix[2], octet).to_string())
        .collect())
}

fn parse_octet(text: &str) -> Option<u8> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Troca `{sub}` e `{loja}` e expande a faixa do modelo para uma loja.
pub fn expand_template(
    template: &str,
    store_code: &str,
    subnet: &str,
) -> Result<Vec<String>, String> {
    if template.contains("{sub}") && subnet.trim().is_empty() {
        return Err("A loja não tem sub-rede para preencher {sub}.".into());
    }
    expand_range(
        &template
            .replace("{sub}", subnet.trim())
            .replace("{loja}", store_code.trim()),
    )
}

#[derive(Clone, Copy)]
enum Part {
    Literal(u8),
    Range(u8, u8),
    Subnet,
    StoreCode,
}

/// Modelo de endereço reconhecido octeto a octeto, para achar a loja e o tipo de um IP solto.
struct Pattern {
    parts: [Part; 4],
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Captured {
    subnet: Option<u8>,
    store_code: Option<u8>,
}

impl Pattern {
    fn parse(template: &str) -> Option<Self> {
        let pieces: Vec<&str> = template.trim().split('.').collect();
        if pieces.len() != 4 {
            return None;
        }
        let mut parts = [Part::Literal(0); 4];
        for (slot, piece) in parts.iter_mut().zip(pieces) {
            *slot = match piece {
                "{sub}" => Part::Subnet,
                "{loja}" => Part::StoreCode,
                _ => match piece.split_once('-') {
                    Some((first, last)) => Part::Range(parse_octet(first)?, parse_octet(last)?),
                    None => Part::Literal(parse_octet(piece)?),
                },
            };
        }
        Some(Self { parts })
    }

    /// Um único endereço por loja, como o concentrador em `.200`.
    fn is_single(&self) -> bool {
        !self
            .parts
            .iter()
            .any(|part| matches!(part, Part::Range(..)))
    }

    fn capture(&self, host: Ipv4Addr) -> Option<Captured> {
        let mut captured = Captured::default();
        for (part, octet) in self.parts.iter().zip(host.octets()) {
            match *part {
                Part::Literal(value) if value == octet => {}
                Part::Range(first, last) if (first..=last).contains(&octet) => {}
                Part::Subnet => captured.subnet = Some(octet),
                Part::StoreCode => captured.store_code = Some(octet),
                _ => return None,
            }
        }
        Some(captured)
    }
}

/// Nome padrão: o tipo seguido do último octeto ("PDV 07"), ou só o tipo se a regra gera um endereço.
pub fn device_name(kind: &str, host: &str, single: bool) -> String {
    if single {
        return kind.to_owned();
    }
    match host.parse::<Ipv4Addr>() {
        Ok(address) => format!("{kind} {:02}", address.octets()[3]),
        Err(_) => format!("{kind} {host}"),
    }
}

fn split_fields(line: &str) -> Vec<String> {
    let separator = [';', '\t', ',']
        .into_iter()
        .find(|separator| line.contains(*separator));
    match separator {
        Some(separator) => line
            .split(separator)
            .map(|field| field.trim().to_owned())
            .collect(),
        None => vec![line.trim().to_owned()],
    }
}

/// Linhas significativas, numeradas a partir de 1; ignora vazias e comentários com `#`.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// Lojas da regra de rede, uma por linha: `código;nome;sub-rede`. Sem sub-rede, vale o código.
pub fn parse_store_lines(text: &str) -> (Vec<StoreSpec>, Vec<LineError>) {
    let mut stores = Vec::new();
    let mut errors = Vec::new();
    for (line, content) in lines(text) {
        let fields = split_fields(content);
        if fields.len() > 3 {
            errors.push(LineError::new(line, "Use código;nome;sub-rede."));
            continue;
        }
        let code = fields[0].clone();
        if code.is_empty() {
            errors.push(LineError::new(line, "Falta o código da loja."));
            continue;
        }
        let subnet = fields
            .get(2)
            .filter(|subnet| !subnet.is_empty())
            .unwrap_or(&code)
            .clone();
        stores.push(StoreSpec {
            line,
            name: fields.get(1).cloned().unwrap_or_default(),
            code,
            subnet,
        });
    }
    (stores, errors)
}

/// Gera os equipamentos de cada loja a partir das linhas da regra.
pub fn generate(rules: &[AddressRule], stores_text: &str) -> Batch {
    let (stores, mut errors) = parse_store_lines(stores_text);
    let mut entries = Vec::new();
    for store in &stores {
        for rule in rules {
            let single = Pattern::parse(&rule.template).is_some_and(|pattern| pattern.is_single());
            match expand_template(&rule.template, &store.code, &store.subnet) {
                Ok(hosts) => entries.extend(hosts.into_iter().map(|host| Entry {
                    line: store.line,
                    store_code: Some(store.code.clone()),
                    name: Some(device_name(&rule.kind, &host, single)),
                    host,
                    kind: Some(rule.kind.clone()),
                    profile_id: rule.profile_id,
                    profile_name: None,
                })),
                Err(message) => errors.push(LineError::new(
                    store.line,
                    format!("Regra {}: {message}", rule.kind),
                )),
            }
        }
    }
    Batch {
        stores,
        entries,
        errors,
        notices: Vec::new(),
    }
}

/// Lista colada: IPs, faixas (`10.113.3.1-12`) ou linhas `loja;ip;tipo;nome`.
pub fn parse_paste(text: &str) -> Batch {
    let mut batch = Batch::default();
    for (line, content) in lines(text) {
        let fields = split_fields(content);
        if fields.len() > 4 {
            batch
                .errors
                .push(LineError::new(line, "Use IP, faixa ou loja;ip;tipo;nome."));
            continue;
        }
        let (store_code, address) = if fields.len() == 1 {
            (None, fields[0].as_str())
        } else {
            (
                Some(fields[0].clone()).filter(|code| !code.is_empty()),
                fields[1].as_str(),
            )
        };
        let hosts = match expand_range(address) {
            Ok(hosts) => hosts,
            // Com a loja informada, aceita nome de máquina além de IP.
            Err(_) if store_code.is_some() && is_hostname(address) => vec![address.to_owned()],
            Err(message) => {
                batch.errors.push(LineError::new(line, message));
                continue;
            }
        };
        let field = |index: usize| fields.get(index).filter(|value| !value.is_empty()).cloned();
        let name = field(3).filter(|_| hosts.len() == 1);
        batch.entries.extend(hosts.into_iter().map(|host| Entry {
            line,
            store_code: store_code.clone(),
            host,
            kind: field(2),
            name: name.clone(),
            profile_id: None,
            profile_name: None,
        }));
    }
    batch
}

fn is_hostname(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
}

/// O que já existe na rede, usado para deduzir lojas e apontar duplicados.
pub struct Fleet<'a> {
    pub stores: &'a [Store],
    pub devices: &'a [Device],
    pub rules: &'a [AddressRule],
    pub profiles: &'a [Profile],
}

/// Confere o lote contra a frota: deduz loja, tipo e perfil pela regra, marca duplicados e erros.
pub fn plan(batch: Batch, fleet: &Fleet) -> Plan {
    let mut planner = Planner {
        fleet,
        patterns: fleet
            .rules
            .iter()
            .filter_map(|rule| Pattern::parse(&rule.template).map(|pattern| (rule, pattern)))
            .collect(),
        known: fleet
            .stores
            .iter()
            .map(|store| store.code.to_lowercase())
            .collect(),
        existing: fleet
            .devices
            .iter()
            .map(|device| device.host.to_lowercase())
            .collect(),
        seen: HashSet::new(),
        plan: Plan {
            errors: batch.errors,
            notices: batch.notices,
            ..Plan::default()
        },
    };
    for spec in batch.stores {
        let code = spec.code.trim().to_owned();
        if code.is_empty() {
            planner
                .plan
                .errors
                .push(LineError::new(spec.line, "Falta o código da loja."));
        } else {
            planner.add_store(StoreSpec {
                code,
                name: spec.name.trim().to_owned(),
                subnet: spec.subnet.trim().to_owned(),
                line: spec.line,
            });
        }
    }
    for entry in batch.entries {
        match planner.device(&entry) {
            Ok(device) => planner.plan.devices.push(device),
            Err(message) => planner
                .plan
                .errors
                .push(LineError::new(entry.line, message)),
        }
    }
    planner.plan
}

struct Planner<'a> {
    fleet: &'a Fleet<'a>,
    patterns: Vec<(&'a AddressRule, Pattern)>,
    /// Códigos de loja em minúsculas, existentes e novos.
    known: HashSet<String>,
    existing: HashSet<String>,
    seen: HashSet<String>,
    plan: Plan,
}

impl Planner<'_> {
    fn add_store(&mut self, spec: StoreSpec) {
        if self.known.insert(spec.code.to_lowercase()) {
            self.plan.stores.push(spec);
        }
    }

    fn device(&mut self, entry: &Entry) -> Result<PlannedDevice, String> {
        let host = entry.host.trim().to_owned();
        if host.is_empty() {
            return Err("Falta o endereço.".into());
        }
        let matched = host.parse::<Ipv4Addr>().ok().and_then(|address| {
            self.patterns.iter().find_map(|(rule, pattern)| {
                pattern
                    .capture(address)
                    .map(|captured| (*rule, pattern.is_single(), captured))
            })
        });
        let store_code = match (&entry.store_code, &matched) {
            (Some(code), _) => code.trim().to_owned(),
            (None, Some((_, _, captured))) => {
                store_for(captured, self.fleet.stores, &self.plan.stores).unwrap_or_else(|| {
                    captured
                        .store_code
                        .or(captured.subnet)
                        .unwrap_or_default()
                        .to_string()
                })
            }
            (None, None) => {
                return Err(format!(
                    "Não foi possível saber a loja de {host}; use loja;ip;tipo."
                ));
            }
        };
        if store_code.is_empty() {
            return Err("Falta o código da loja.".into());
        }
        let profile_ids = profiles_for(
            entry,
            matched.as_ref().map(|(rule, ..)| *rule),
            self.fleet.profiles,
        )?;
        self.add_store(StoreSpec {
            line: entry.line,
            code: store_code.clone(),
            name: String::new(),
            subnet: matched
                .as_ref()
                .and_then(|(_, _, captured)| captured.subnet)
                .map(|octet| octet.to_string())
                .unwrap_or_default(),
        });
        let kind = entry
            .kind
            .clone()
            .or_else(|| matched.as_ref().map(|(rule, ..)| rule.kind.clone()))
            .unwrap_or_else(|| DEFAULT_KIND.to_owned());
        let single = matched
            .as_ref()
            .is_some_and(|(rule, single, _)| *single && rule.kind == kind);
        let name = entry
            .name
            .clone()
            .unwrap_or_else(|| device_name(&kind, &host, single));
        let key = host.to_lowercase();
        let status = if self.existing.contains(&key) {
            DeviceStatus::Exists
        } else if self.seen.insert(key) {
            DeviceStatus::New
        } else {
            DeviceStatus::Repeated
        };
        Ok(PlannedDevice {
            line: entry.line,
            store_code,
            name,
            kind,
            host,
            profile_ids,
            status,
        })
    }
}

/// Loja cujo código ou sub-rede bate com o octeto capturado pela regra.
fn store_for(captured: &Captured, stores: &[Store], new: &[StoreSpec]) -> Option<String> {
    let same = |text: &str, octet: Option<u8>| octet.is_some() && parse_octet(text) == octet;
    stores
        .iter()
        .map(|store| (&store.code, &store.subnet))
        .chain(new.iter().map(|spec| (&spec.code, &spec.subnet)))
        .find(|(code, subnet)| same(code, captured.store_code) || same(subnet, captured.subnet))
        .map(|(code, _)| code.clone())
}

fn profiles_for(
    entry: &Entry,
    rule: Option<&AddressRule>,
    profiles: &[Profile],
) -> Result<Vec<i64>, String> {
    if let Some(id) = entry.profile_id {
        return if profiles.iter().any(|profile| profile.id == id) {
            Ok(vec![id])
        } else {
            Err("O perfil escolhido não existe mais nesta rede.".into())
        };
    }
    if let Some(name) = &entry.profile_name {
        return profiles
            .iter()
            .find(|profile| profile.name.eq_ignore_ascii_case(name.trim()))
            .map(|profile| vec![profile.id])
            .ok_or_else(|| format!("O perfil {name} não existe nesta rede."));
    }
    if let Some(id) = rule.and_then(|rule| rule.profile_id) {
        return Ok(vec![id]);
    }
    Ok(match profiles {
        [only] => vec![only.id],
        _ => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Engine;

    fn rules() -> Vec<AddressRule> {
        vec![
            AddressRule {
                kind: "PDV".into(),
                template: "10.{sub}.3.1-50".into(),
                profile_id: Some(1),
            },
            AddressRule {
                kind: "Self".into(),
                template: "10.{sub}.3.101-130".into(),
                profile_id: Some(1),
            },
            AddressRule {
                kind: "Concentrador".into(),
                template: "10.{sub}.3.200".into(),
                profile_id: Some(2),
            },
        ]
    }

    fn profiles() -> Vec<Profile> {
        [(1, "Econect"), (2, "Concentrador")]
            .into_iter()
            .map(|(id, name)| Profile {
                id,
                chain_id: 1,
                name: name.into(),
                engine: Engine::Mysql,
                port: 3306,
                database: "pdv".into(),
                username: "leitura".into(),
                tls: false,
            })
            .collect()
    }

    fn store(id: i64, code: &str, subnet: &str) -> Store {
        Store {
            id,
            chain_id: 1,
            code: code.into(),
            name: format!("Loja {code}"),
            subnet: subnet.into(),
        }
    }

    #[test]
    fn expands_ranges_and_rejects_bad_addresses() {
        assert_eq!(
            expand_range("10.113.3.1-3").unwrap(),
            ["10.113.3.1", "10.113.3.2", "10.113.3.3"]
        );
        assert_eq!(expand_range(" 10.1.1.200 ").unwrap(), ["10.1.1.200"]);
        assert_eq!(expand_range("10.1.1.0-255").unwrap().len(), 256);
        for bad in [
            "10.1.1",
            "10.1.1.256",
            "10.1.1.5-2",
            "10.1.a.1",
            "10.1.1.-1",
            "10.1.1.1-",
        ] {
            assert!(expand_range(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn template_fills_subnet_and_store_code() {
        assert_eq!(
            expand_template("10.{sub}.3.1-2", "A1", "25").unwrap(),
            ["10.25.3.1", "10.25.3.2"]
        );
        assert_eq!(
            expand_template("10.{loja}.3.200", "101", "").unwrap(),
            ["10.101.3.200"]
        );
        assert!(expand_template("10.{sub}.3.1", "101", " ").is_err());
    }

    #[test]
    fn rule_generates_devices_per_store() {
        let batch = generate(
            &rules(),
            "101;Centro;101\n102;Jardim América\n\n# comentário\n;sem código",
        );
        assert_eq!(batch.stores.len(), 2);
        assert_eq!(batch.stores[1].subnet, "102");
        assert_eq!(batch.errors, [LineError::new(5, "Falta o código da loja.")]);
        assert_eq!(batch.entries.len(), 2 * (50 + 30 + 1));
        let concentrator = batch
            .entries
            .iter()
            .find(|entry| entry.host == "10.101.3.200")
            .unwrap();
        assert_eq!(concentrator.name.as_deref(), Some("Concentrador"));
        assert_eq!(concentrator.profile_id, Some(2));
        assert_eq!(batch.entries[6].name.as_deref(), Some("PDV 07"));
    }

    #[test]
    fn rule_reports_store_without_subnet() {
        let rules = vec![AddressRule {
            kind: "PDV".into(),
            template: "10.{sub}.3.1-2".into(),
            profile_id: None,
        }];
        let batch = generate(&rules, "LJ;Loja sem número;");
        assert_eq!(batch.errors.len(), 1);
        assert!(batch.errors[0].message.starts_with("Regra PDV:"));
    }

    #[test]
    fn paste_accepts_ips_ranges_and_store_lines() {
        let batch = parse_paste(
            "10.113.3.1-12\n10.113.3.200\n114;10.114.3.5;PDV\n114;caixa-6.loja;Self;Caixa 6\n10.1.1\na;b;c;d;e",
        );
        assert_eq!(batch.entries.len(), 15);
        assert_eq!(batch.entries[14].host, "caixa-6.loja");
        assert_eq!(batch.entries[14].name.as_deref(), Some("Caixa 6"));
        assert_eq!(
            batch
                .errors
                .iter()
                .map(|error| error.line)
                .collect::<Vec<_>>(),
            [5, 6]
        );
    }

    #[test]
    fn plan_deduces_store_and_kind_from_rule() {
        use DeviceStatus::{Exists, New, Repeated};
        let stores = [store(1, "113", "113")];
        let devices = [Device {
            id: 1,
            store_id: 1,
            name: "PDV 01".into(),
            kind: "PDV".into(),
            host: "10.113.3.1".into(),
            profile_ids: vec![1],
        }];
        let (rules, profiles) = (rules(), profiles());
        let fleet = Fleet {
            stores: &stores,
            devices: &devices,
            rules: &rules,
            profiles: &profiles,
        };
        let plan = plan(
            parse_paste(
                "10.113.3.1-3\n10.113.3.2\n10.113.3.200\n10.115.3.7\n192.168.0.1\n116;192.168.0.2",
            ),
            &fleet,
        );
        let status: Vec<_> = plan.devices.iter().map(|device| device.status).collect();
        assert_eq!(status, [Exists, New, New, Repeated, New, New, New]);
        let concentrator = &plan.devices[4];
        assert_eq!(
            (concentrator.kind.as_str(), concentrator.name.as_str()),
            ("Concentrador", "Concentrador")
        );
        assert_eq!(concentrator.profile_ids, [2]);
        assert_eq!(plan.devices[5].store_code, "115");
        assert_eq!(plan.devices[6].kind, "PDV");
        assert_eq!(plan.devices[6].profile_ids, Vec::<i64>::new());
        assert_eq!(
            plan.stores
                .iter()
                .map(|spec| (spec.code.as_str(), spec.subnet.as_str()))
                .collect::<Vec<_>>(),
            [("115", "115"), ("116", "")]
        );
        assert_eq!(plan.errors.len(), 1);
        assert!(plan.errors[0].message.contains("192.168.0.1"));
        assert_eq!(plan.new_devices().count(), 5);
    }

    #[test]
    fn plan_matches_store_by_subnet_and_checks_profile_names() {
        let stores = [store(1, "Centro", "25")];
        let (rules, profiles) = (rules(), profiles());
        let fleet = Fleet {
            stores: &stores,
            devices: &[],
            rules: &rules,
            profiles: &profiles,
        };
        let mut batch = parse_paste("10.25.3.4");
        batch.entries.push(Entry {
            line: 2,
            store_code: Some("Centro".into()),
            host: "10.25.3.5".into(),
            profile_name: Some("econect".into()),
            ..Entry::default()
        });
        batch.entries.push(Entry {
            line: 3,
            store_code: Some("Centro".into()),
            host: "10.25.3.6".into(),
            profile_name: Some("Linx".into()),
            ..Entry::default()
        });
        let plan = plan(batch, &fleet);
        assert_eq!(plan.stores, []);
        assert_eq!(plan.devices[0].store_code, "Centro");
        assert_eq!(plan.devices[1].profile_ids, [1]);
        assert_eq!(
            plan.errors,
            [LineError::new(3, "O perfil Linx não existe nesta rede.")]
        );
    }
}
