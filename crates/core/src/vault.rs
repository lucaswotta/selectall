//! Senhas no cofre do sistema operacional: Secret Service no Linux,
//! Gerenciador de Credenciais no Windows, Keychain no macOS.

const SERVICE: &str = "io.github.lucaswotta.selectall";

/// Chave de um perfil. `install_id` separa instalações que compartilham o mesmo cofre.
pub fn profile_key(install_id: &str, profile_id: i64) -> String {
    format!("{install_id}:profile:{profile_id}")
}

pub fn write(key: &str, password: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, key)
        .and_then(|entry| entry.set_password(password))
        .map_err(|error| format!("O cofre do sistema recusou a senha: {error}"))
}

/// `Ok(None)` quando não há senha guardada; erro quando o cofre não responde.
pub fn read(key: &str) -> Result<Option<String>, String> {
    match keyring::Entry::new(SERVICE, key).and_then(|entry| entry.get_password()) {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("O cofre do sistema não respondeu: {error}")),
    }
}

pub fn delete(key: &str) -> Result<(), String> {
    match keyring::Entry::new(SERVICE, key).and_then(|entry| entry.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("O cofre do sistema não removeu a senha: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "precisa de um cofre do sistema desbloqueado"]
    fn roundtrip() {
        let key = profile_key(&format!("test-{}", std::process::id()), 1);
        write(&key, "valor-de-teste").unwrap();
        assert_eq!(read(&key).unwrap().as_deref(), Some("valor-de-teste"));
        delete(&key).unwrap();
        assert_eq!(read(&key).unwrap(), None);
    }
}
